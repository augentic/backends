//! Functions and types to support modifier queries against Azure Table Storage.
//! Only INSERT, UPDATE, and DELETE are supported.

use anyhow::{anyhow, bail};
use base64ct::{Base64, Encoding};
use qwasr_wasi_sql::{DataType, Field};
use serde_json::Value;

#[derive(Debug)]
pub enum ExecAction {
    Insert,
    Update,
    Delete,
}

#[derive(Debug)]
pub struct ExecPhrase {
    pub action: ExecAction,
    pub partition: String,
    pub row: String,
    pub entity: Vec<Field>,
}

impl ExecPhrase {
    pub fn parse(query: &str, params: &[DataType]) -> anyhow::Result<Self> {
        let query_upper = query.trim().to_uppercase();

        // Determine the action
        let action = if query_upper.starts_with("INSERT") {
            ExecAction::Insert
        } else if query_upper.starts_with("UPDATE") {
            ExecAction::Update
        } else if query_upper.starts_with("DELETE") {
            ExecAction::Delete
        } else {
            bail!("only INSERT, UPDATE, and DELETE queries are supported");
        };

        // Parse fields based on action, using original parameter types
        let (entity, partition, row) = match action {
            ExecAction::Insert => Self::parse_insert(query, params)?,
            ExecAction::Update => Self::parse_update(query, params)?,
            ExecAction::Delete => Self::parse_delete(query, params)?,
        };

        Ok(Self {
            action,
            partition,
            row,
            entity,
        })
    }

    fn parse_insert(
        query: &str, params: &[DataType],
    ) -> anyhow::Result<(Vec<Field>, String, String)> {
        // Parse INSERT INTO table (col1, col2) VALUES ($1, $2)
        let query_upper = query.to_uppercase();

        // Find VALUES keyword
        let values_pos = query_upper
            .find("VALUES")
            .ok_or_else(|| anyhow!("INSERT query must contain VALUES clause"))?;

        // Extract column names from INSERT clause
        let insert_part = &query[..values_pos];
        let columns = Self::extract_columns(insert_part)?;

        // Extract parameter placeholders from VALUES clause
        let values_part = &query[values_pos + 6..]; // Skip "VALUES"
        let param_indices = Self::extract_param_placeholders(values_part)?;

        // Check that columns and values match
        if columns.len() != param_indices.len() {
            bail!(
                "number of columns ({}) does not match number of values ({})",
                columns.len(),
                param_indices.len()
            );
        }

        // Create Field entries using original parameter types
        let mut fields = Vec::new();
        for (col, param_idx) in columns.iter().zip(param_indices.iter()) {
            if *param_idx >= params.len() {
                bail!(
                    "parameter ${} referenced but only {} parameters provided",
                    param_idx + 1,
                    params.len()
                );
            }
            fields.push(Field {
                name: col.clone(),
                value: params[*param_idx].clone(),
            });
        }

        // Extract PartitionKey and RowKey
        let (partition, row) = Self::extract_partition_and_row_keys(&fields, "INSERT")?;

        Ok((fields, partition, row))
    }

    fn parse_update(
        query: &str, params: &[DataType],
    ) -> anyhow::Result<(Vec<Field>, String, String)> {
        // Parse UPDATE table SET col1 = $1, col2 = $2 WHERE ...
        let query_upper = query.to_uppercase();

        // Find SET keyword
        let set_pos = query_upper
            .find(" SET ")
            .ok_or_else(|| anyhow!("UPDATE query must contain SET clause"))?;

        // Find WHERE keyword (optional)
        let set_end = query_upper.find(" WHERE ").unwrap_or(query.len());

        // Extract SET clause
        let set_part = &query[set_pos + 5..set_end].trim();

        // Parse column = $N pairs
        let mut fields = Vec::new();
        for pair in set_part.split(',') {
            let parts: Vec<&str> = pair.split('=').map(str::trim).collect();
            if parts.len() != 2 {
                bail!("invalid SET clause: expected 'column = value' format");
            }

            let col_name = parts[0].to_string();
            let value_part = parts[1];

            // Check if it's a parameter placeholder
            if let Some(param_idx) = Self::parse_param_placeholder(value_part) {
                if param_idx >= params.len() {
                    bail!(
                        "Parameter ${} referenced but only {} parameters provided",
                        param_idx + 1,
                        params.len()
                    );
                }
                fields.push(Field {
                    name: col_name,
                    value: params[param_idx].clone(),
                });
            } else {
                bail!("UPDATE SET clause must use parameter placeholders (e.g., $1, $2)");
            }
        }

        // Parse WHERE clause to extract PartitionKey and RowKey
        if !query_upper.contains(" WHERE ") {
            bail!("UPDATE query must have a WHERE clause to specify the entity to update");
        }
        let where_fields = Self::parse_where_clause(query, &query_upper, params)?;
        if where_fields.is_empty() {
            bail!(
                "UPDATE query must have a non-empty WHERE clause to specify the entity to update"
            );
        }

        // Validate WHERE clause only contains PartitionKey and RowKey
        Self::validate_where_clause_simple(&where_fields, "UPDATE")?;

        // Extract PartitionKey and RowKey
        let (partition, row) = Self::extract_partition_and_row_keys(&where_fields, "UPDATE")?;

        Ok((fields, partition, row))
    }

    fn parse_delete(
        query: &str, params: &[DataType],
    ) -> anyhow::Result<(Vec<Field>, String, String)> {
        // DELETE queries don't really need entity fields but we parse the
        // WHERE clause to extract the PartitionKey and RowKey.
        let query_upper = query.to_uppercase();

        // Parse WHERE clause to extract PartitionKey and RowKey
        if !query_upper.contains(" WHERE ") {
            bail!("DELETE query must have a WHERE clause to specify the entity to delete");
        }
        let where_fields = Self::parse_where_clause(query, &query_upper, params)?;
        if where_fields.is_empty() {
            bail!(
                "DELETE query must have a non-empty WHERE clause to specify the entity to delete"
            );
        }

        // Validate WHERE clause only contains PartitionKey and RowKey
        Self::validate_where_clause_simple(&where_fields, "DELETE")?;

        // Extract PartitionKey and RowKey
        let (partition, row) = Self::extract_partition_and_row_keys(&where_fields, "DELETE")?;

        Ok((Vec::new(), partition, row))
    }

    fn extract_columns(insert_part: &str) -> anyhow::Result<Vec<String>> {
        // Find the opening parenthesis for column names
        let open_paren = insert_part
            .find('(')
            .ok_or_else(|| anyhow!("INSERT query must specify column names in parentheses"))?;
        let close_paren = insert_part
            .rfind(')')
            .ok_or_else(|| anyhow!("INSERT query must specify column names in parentheses"))?;

        let columns_str = &insert_part[open_paren + 1..close_paren];
        let columns: Vec<String> = columns_str.split(',').map(|s| s.trim().to_string()).collect();

        Ok(columns)
    }

    fn extract_param_placeholders(values_part: &str) -> anyhow::Result<Vec<usize>> {
        // Find the opening and closing parentheses for values
        let open_paren = values_part
            .find('(')
            .ok_or_else(|| anyhow!("VALUES clause must contain values in parentheses"))?;
        let close_paren = values_part
            .rfind(')')
            .ok_or_else(|| anyhow!("VALUES clause must contain values in parentheses"))?;

        let values_str = &values_part[open_paren + 1..close_paren];

        // Split by comma and extract parameter indices
        let mut param_indices = Vec::new();
        for value in values_str.split(',') {
            let trimmed = value.trim();
            if let Some(idx) = Self::parse_param_placeholder(trimmed) {
                param_indices.push(idx);
            } else {
                bail!(
                    "VALUES clause must use parameter placeholders (e.g., $1, $2), found: {trimmed}"
                );
            }
        }

        Ok(param_indices)
    }

    fn parse_param_placeholder(s: &str) -> Option<usize> {
        // Parse $1, $2, etc. and return 0-based index
        s.strip_prefix('$')
            .and_then(|num_str| num_str.parse::<usize>().ok())
            .and_then(|n| n.checked_sub(1))
    }

    /// Parse WHERE clause and extract fields from column = $N conditions
    fn parse_where_clause(
        query: &str, query_upper: &str, params: &[DataType],
    ) -> anyhow::Result<Vec<Field>> {
        let mut where_fields = Vec::new();

        if let Some(where_pos) = query_upper.find(" WHERE ") {
            let filter_str = &query[where_pos + 7..].trim();
            if filter_str.is_empty() {
                bail!("WHERE clause cannot be empty");
            }
            // Parse the WHERE clause for column = $N conditions
            for part in filter_str.split(" AND ") {
                let part = part.trim();
                // Parse column = $N
                let eq_parts: Vec<&str> = part.split('=').map(str::trim).collect();
                if eq_parts.len() == 2 {
                    let col_name = eq_parts[0].to_string();
                    let value_part = eq_parts[1];

                    // Check if it's a parameter placeholder
                    if let Some(param_idx) = Self::parse_param_placeholder(value_part) {
                        if param_idx >= params.len() {
                            bail!(
                                "Parameter ${} referenced but only {} parameters provided",
                                param_idx + 1,
                                params.len()
                            );
                        }
                        where_fields.push(Field {
                            name: col_name,
                            value: params[param_idx].clone(),
                        });
                    }
                }
            }
        }

        Ok(where_fields)
    }

    /// Extract and validate `PartitionKey` and `RowKey` from a fields vector
    fn extract_partition_and_row_keys(
        fields: &[Field], query_type: &str,
    ) -> anyhow::Result<(String, String)> {
        let partition_field = fields
            .iter()
            .find(|f| f.name == "PartitionKey")
            .ok_or_else(|| anyhow!("{query_type} query must specify PartitionKey column"))?;
        let partition = match &partition_field.value {
            DataType::Str(Some(s)) => s.clone(),
            _ => bail!("PartitionKey must be a non-null string"),
        };

        let row_field = fields
            .iter()
            .find(|f| f.name == "RowKey")
            .ok_or_else(|| anyhow!("{query_type} query must specify RowKey column"))?;
        let row = match &row_field.value {
            DataType::Str(Some(s)) => s.clone(),
            _ => bail!("RowKey must be a non-null string"),
        };

        Ok((partition, row))
    }

    /// Validate that WHERE clause only contains `PartitionKey` and `RowKey` conditions
    fn validate_where_clause_simple(
        where_fields: &[Field], query_type: &str,
    ) -> anyhow::Result<()> {
        if where_fields.len() > 2 {
            bail!(
                "{query_type} query has unsupported conditions in WHERE clause - only PartitionKey and RowKey equality conditions are supported"
            );
        }
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    pub fn entity_to_json(&self) -> anyhow::Result<Option<Value>> {
        if self.entity.is_empty() {
            Ok(None)
        } else {
            let mut map = serde_json::Map::new();
            // Add entity fields with OData metadata for types that cannot be inferred
            for field in &self.entity {
                match &field.value {
                    DataType::Binary(opt) => {
                        if let Some(data) = opt {
                            map.insert(
                                field.name.clone(),
                                Value::String(Base64::encode_string(data)),
                            );
                            map.insert(
                                format!("{}@odata.type", field.name),
                                Value::String("Edm.Binary".to_string()),
                            );
                        }
                        // Skip null values - don't insert anything
                    }
                    DataType::Boolean(opt) => {
                        if let Some(val) = opt {
                            map.insert(field.name.clone(), Value::Bool(*val));
                        }
                        // No @odata.type needed - can be inferred from JSON bool
                    }
                    DataType::Int32(opt) => {
                        if let Some(n) = opt {
                            map.insert(field.name.clone(), serde_json::json!(n));
                        }
                        // No @odata.type needed - can be inferred from JSON integer
                    }
                    DataType::Int64(opt) => {
                        if let Some(n) = opt {
                            map.insert(field.name.clone(), serde_json::json!(n));
                            map.insert(
                                format!("{}@odata.type", field.name),
                                Value::String("Edm.Int64".to_string()),
                            );
                        }
                    }
                    DataType::Uint32(opt) => {
                        // Azure Table Storage doesn't support unsigned integers - convert to Int64
                        if let Some(n) = opt {
                            map.insert(field.name.clone(), serde_json::json!(i64::from(*n)));
                            map.insert(
                                format!("{}@odata.type", field.name),
                                Value::String("Edm.Int64".to_string()),
                            );
                        }
                    }
                    DataType::Uint64(opt) => {
                        // Azure Table Storage doesn't support unsigned integers
                        if let Some(n) = opt {
                            if *n > i64::MAX as u64 {
                                bail!(
                                    "Uint64 value {n} exceeds maximum Int64 value and cannot be stored in Azure Table Storage"
                                );
                            }
                            map.insert(field.name.clone(), serde_json::json!((*n).cast_signed()));
                            map.insert(
                                format!("{}@odata.type", field.name),
                                Value::String("Edm.Int64".to_string()),
                            );
                        }
                    }
                    DataType::Float(opt) => {
                        if let Some(f) = opt {
                            map.insert(field.name.clone(), serde_json::json!(f64::from(*f)));
                            map.insert(
                                format!("{}@odata.type", field.name),
                                Value::String("Edm.Double".to_string()),
                            );
                        }
                    }
                    DataType::Double(opt) => {
                        if let Some(f) = opt {
                            map.insert(field.name.clone(), serde_json::json!(f));
                        }
                        // No @odata.type needed - can be inferred from JSON f64
                    }
                    DataType::Str(opt) => {
                        if let Some(s) = opt {
                            map.insert(field.name.clone(), Value::String(s.clone()));
                        }
                        // No @odata.type needed - can be inferred from JSON string
                    }
                    DataType::Date(opt) | DataType::Time(opt) | DataType::Timestamp(opt) => {
                        if let Some(s) = opt {
                            map.insert(field.name.clone(), Value::String(s.clone()));
                            map.insert(
                                format!("{}@odata.type", field.name),
                                Value::String("Edm.DateTime".to_string()),
                            );
                        }
                    }
                }
            }

            Ok(Some(Value::Object(map)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_to_json_empty() {
        let exec_phrase = ExecPhrase {
            action: ExecAction::Delete,
            partition: String::new(),
            row: String::new(),
            entity: Vec::new(),
        };

        let result = exec_phrase.entity_to_json().unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn entity_to_json_with_partition_and_row_keys() {
        let exec_phrase = ExecPhrase {
            action: ExecAction::Insert,
            partition: "partition1".to_string(),
            row: "row1".to_string(),
            entity: vec![
                Field {
                    name: "PartitionKey".to_string(),
                    value: DataType::Str(Some("partition1".to_string())),
                },
                Field {
                    name: "RowKey".to_string(),
                    value: DataType::Str(Some("row1".to_string())),
                },
                Field {
                    name: "Name".to_string(),
                    value: DataType::Str(Some("Alice".to_string())),
                },
            ],
        };

        let result = exec_phrase.entity_to_json().unwrap().unwrap();
        let obj = result.as_object().unwrap();

        assert_eq!(obj.get("PartitionKey").unwrap().as_str().unwrap(), "partition1");
        assert_eq!(obj.get("RowKey").unwrap().as_str().unwrap(), "row1");
    }

    #[test]
    fn entity_to_json_inferable_types_no_metadata() {
        let exec_phrase = ExecPhrase {
            action: ExecAction::Insert,
            partition: "partition1".to_string(),
            row: "row1".to_string(),
            entity: vec![
                Field {
                    name: "PartitionKey".to_string(),
                    value: DataType::Str(Some("partition1".to_string())),
                },
                Field {
                    name: "RowKey".to_string(),
                    value: DataType::Str(Some("row1".to_string())),
                },
                Field {
                    name: "stringField".to_string(),
                    value: DataType::Str(Some("test".to_string())),
                },
                Field {
                    name: "boolField".to_string(),
                    value: DataType::Boolean(Some(true)),
                },
                Field {
                    name: "intField".to_string(),
                    value: DataType::Int32(Some(42)),
                },
                Field {
                    name: "doubleField".to_string(),
                    value: DataType::Double(Some(3.94)),
                },
            ],
        };

        let result = exec_phrase.entity_to_json().unwrap().unwrap();
        let obj = result.as_object().unwrap();

        // Check values are correct
        assert_eq!(obj.get("stringField").unwrap().as_str().unwrap(), "test");
        assert!(obj.get("boolField").unwrap().as_bool().unwrap());
        assert_eq!(obj.get("intField").unwrap().as_i64().unwrap(), 42);
        assert!((obj.get("doubleField").unwrap().as_f64().unwrap() - 3.94).abs() < f64::EPSILON);

        // Check that NO @odata.type metadata is present for inferable types
        assert!(obj.get("stringField@odata.type").is_none());
        assert!(obj.get("boolField@odata.type").is_none());
        assert!(obj.get("intField@odata.type").is_none());
        assert!(obj.get("doubleField@odata.type").is_none());
    }

    #[test]
    fn entity_to_json_non_inferable_types_with_metadata() {
        let exec_phrase = ExecPhrase {
            action: ExecAction::Insert,
            partition: "partition1".to_string(),
            row: "row1".to_string(),
            entity: vec![
                Field {
                    name: "PartitionKey".to_string(),
                    value: DataType::Str(Some("partition1".to_string())),
                },
                Field {
                    name: "RowKey".to_string(),
                    value: DataType::Str(Some("row1".to_string())),
                },
                Field {
                    name: "longField".to_string(),
                    value: DataType::Int64(Some(9_223_372_036_854_775_807)),
                },
                Field {
                    name: "timestampField".to_string(),
                    value: DataType::Timestamp(Some("2026-01-30T12:00:00Z".to_string())),
                },
                Field {
                    name: "floatField".to_string(),
                    value: DataType::Float(Some(1.5)),
                },
            ],
        };

        let result = exec_phrase.entity_to_json().unwrap().unwrap();
        let obj = result.as_object().unwrap();

        // Check values
        assert_eq!(obj.get("longField").unwrap().as_i64().unwrap(), 9_223_372_036_854_775_807);
        assert_eq!(obj.get("timestampField").unwrap().as_str().unwrap(), "2026-01-30T12:00:00Z");
        assert!((obj.get("floatField").unwrap().as_f64().unwrap() - 1.5).abs() < f64::EPSILON);

        // Check that @odata.type metadata IS present for non-inferable types
        assert_eq!(obj.get("longField@odata.type").unwrap().as_str().unwrap(), "Edm.Int64");
        assert_eq!(obj.get("timestampField@odata.type").unwrap().as_str().unwrap(), "Edm.DateTime");
        assert_eq!(obj.get("floatField@odata.type").unwrap().as_str().unwrap(), "Edm.Double");
    }

    #[test]
    fn entity_to_json_various_types() {
        let exec_phrase = ExecPhrase {
            action: ExecAction::Insert,
            partition: "partition1".to_string(),
            row: "row1".to_string(),
            entity: vec![
                Field {
                    name: "PartitionKey".to_string(),
                    value: DataType::Str(Some("partition1".to_string())),
                },
                Field {
                    name: "RowKey".to_string(),
                    value: DataType::Str(Some("row1".to_string())),
                },
                Field {
                    name: "binaryField".to_string(),
                    value: DataType::Binary(Some(b"Hello World".to_vec())),
                },
                Field {
                    name: "dateField".to_string(),
                    value: DataType::Date(Some("2026-01-30".to_string())),
                },
                Field {
                    name: "timeField".to_string(),
                    value: DataType::Time(Some("12:00:00".to_string())),
                },
                Field {
                    name: "nullString".to_string(),
                    value: DataType::Str(None),
                },
                Field {
                    name: "nullInt".to_string(),
                    value: DataType::Int32(None),
                },
                Field {
                    name: "nullBinary".to_string(),
                    value: DataType::Binary(None),
                },
                Field {
                    name: "uint32Field".to_string(),
                    value: DataType::Uint32(Some(4_294_967_295)),
                },
                Field {
                    name: "uint64Field".to_string(),
                    value: DataType::Uint64(Some(1000)),
                },
            ],
        };

        let result = exec_phrase.entity_to_json().unwrap().unwrap();
        let obj = result.as_object().unwrap();

        // Check base64 encoded value
        assert_eq!(obj.get("binaryField").unwrap().as_str().unwrap(), "SGVsbG8gV29ybGQ=");
        assert_eq!(obj.get("binaryField@odata.type").unwrap().as_str().unwrap(), "Edm.Binary");
        assert_eq!(obj.get("dateField").unwrap().as_str().unwrap(), "2026-01-30");
        assert_eq!(obj.get("dateField@odata.type").unwrap().as_str().unwrap(), "Edm.DateTime");
        assert_eq!(obj.get("timeField").unwrap().as_str().unwrap(), "12:00:00");
        assert_eq!(obj.get("timeField@odata.type").unwrap().as_str().unwrap(), "Edm.DateTime");

        // Null values should be absent from the JSON, not present with null value
        assert!(obj.get("nullString").is_none());
        assert!(obj.get("nullInt").is_none());
        assert!(obj.get("nullBinary").is_none());

        // Metadata should also be absent when the value is null
        assert!(obj.get("nullBinary@odata.type").is_none());

        // Uint32 should be converted to Int64
        assert_eq!(obj.get("uint32Field").unwrap().as_i64().unwrap(), 4_294_967_295);
        assert_eq!(obj.get("uint32Field@odata.type").unwrap().as_str().unwrap(), "Edm.Int64");

        // Uint64 should be converted to Int64
        assert_eq!(obj.get("uint64Field").unwrap().as_i64().unwrap(), 1000);
        assert_eq!(obj.get("uint64Field@odata.type").unwrap().as_str().unwrap(), "Edm.Int64");
    }

    #[test]
    fn entity_to_json_uint64_overflow() {
        let exec_phrase = ExecPhrase {
            action: ExecAction::Insert,
            partition: "partition1".to_string(),
            row: "row1".to_string(),
            entity: vec![
                Field {
                    name: "PartitionKey".to_string(),
                    value: DataType::Str(Some("partition1".to_string())),
                },
                Field {
                    name: "RowKey".to_string(),
                    value: DataType::Str(Some("row1".to_string())),
                },
                Field {
                    name: "hugeUint64".to_string(),
                    value: DataType::Uint64(Some(u64::MAX)),
                },
            ],
        };

        let result = exec_phrase.entity_to_json();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("exceeds maximum Int64 value"));
    }

    #[test]
    fn parse_insert_valid() {
        let query = "INSERT INTO table (PartitionKey, RowKey, Name, Age) VALUES ($1, $2, $3, $4)";
        let params = vec![
            DataType::Str(Some("part1".to_string())),
            DataType::Str(Some("row1".to_string())),
            DataType::Str(Some("Alice".to_string())),
            DataType::Int32(Some(30)),
        ];

        let result = ExecPhrase::parse(query, &params).unwrap();

        assert_eq!(result.partition, "part1");
        assert_eq!(result.row, "row1");
        assert_eq!(result.entity.len(), 4);
        assert_eq!(result.entity[0].name, "PartitionKey");
        assert_eq!(result.entity[2].name, "Name");
    }

    #[test]
    fn parse_insert_missing_partition_key() {
        let query = "INSERT INTO table (RowKey, Name) VALUES ($1, $2)";
        let params =
            vec![DataType::Str(Some("row1".to_string())), DataType::Str(Some("Alice".to_string()))];

        let result = ExecPhrase::parse(query, &params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("PartitionKey"));
    }

    #[test]
    fn parse_insert_missing_row_key() {
        let query = "INSERT INTO table (PartitionKey, Name) VALUES ($1, $2)";
        let params = vec![
            DataType::Str(Some("part1".to_string())),
            DataType::Str(Some("Alice".to_string())),
        ];

        let result = ExecPhrase::parse(query, &params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("RowKey"));
    }

    #[test]
    fn parse_insert_mismatched_columns_values() {
        let query = "INSERT INTO table (PartitionKey, RowKey, Name) VALUES ($1, $2)";
        let params =
            vec![DataType::Str(Some("part1".to_string())), DataType::Str(Some("row1".to_string()))];

        let result = ExecPhrase::parse(query, &params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("does not match"));
    }

    #[test]
    fn parse_insert_out_of_bounds_param() {
        let query = "INSERT INTO table (PartitionKey, RowKey, Name) VALUES ($1, $2, $5)";
        let params =
            vec![DataType::Str(Some("part1".to_string())), DataType::Str(Some("row1".to_string()))];

        let result = ExecPhrase::parse(query, &params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("parameter $5"));
    }

    #[test]
    fn parse_update_valid() {
        let query = "UPDATE table SET Name = $1, Age = $2 WHERE PartitionKey = $3 AND RowKey = $4";
        let params = vec![
            DataType::Str(Some("Bob".to_string())),
            DataType::Int32(Some(25)),
            DataType::Str(Some("part1".to_string())),
            DataType::Str(Some("row1".to_string())),
        ];

        let result = ExecPhrase::parse(query, &params).unwrap();

        assert_eq!(result.partition, "part1");
        assert_eq!(result.row, "row1");
        assert_eq!(result.entity.len(), 2);
        assert_eq!(result.entity[0].name, "Name");
        assert_eq!(result.entity[1].name, "Age");
    }

    #[test]
    fn parse_update_missing_where_clause() {
        let query = "UPDATE table SET Name = $1";
        let params = vec![DataType::Str(Some("Bob".to_string()))];

        let result = ExecPhrase::parse(query, &params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("WHERE clause"));
    }

    #[test]
    fn parse_update_empty_where_clause() {
        let query = "UPDATE table SET Name = $1 WHERE ";
        let params = vec![DataType::Str(Some("Bob".to_string()))];

        let result = ExecPhrase::parse(query, &params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("WHERE clause cannot be empty"));
    }

    #[test]
    fn parse_update_missing_partition_key_in_where() {
        let query = "UPDATE table SET Name = $1 WHERE RowKey = $2";
        let params =
            vec![DataType::Str(Some("Bob".to_string())), DataType::Str(Some("row1".to_string()))];

        let result = ExecPhrase::parse(query, &params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("PartitionKey"));
    }

    #[test]
    fn parse_update_missing_row_key_in_where() {
        let query = "UPDATE table SET Name = $1 WHERE PartitionKey = $2";
        let params =
            vec![DataType::Str(Some("Bob".to_string())), DataType::Str(Some("part1".to_string()))];

        let result = ExecPhrase::parse(query, &params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("RowKey"));
    }

    #[test]
    fn parse_update_extra_where_conditions() {
        let query =
            "UPDATE table SET Name = $1 WHERE PartitionKey = $2 AND RowKey = $3 AND Age = $4";
        let params = vec![
            DataType::Str(Some("Bob".to_string())),
            DataType::Str(Some("part1".to_string())),
            DataType::Str(Some("row1".to_string())),
            DataType::Int32(Some(30)),
        ];

        let result = ExecPhrase::parse(query, &params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unsupported conditions"));
    }

    #[test]
    fn parse_update_out_of_bounds_param() {
        let query = "UPDATE table SET Name = $5 WHERE PartitionKey = $1 AND RowKey = $2";
        let params =
            vec![DataType::Str(Some("part1".to_string())), DataType::Str(Some("row1".to_string()))];

        let result = ExecPhrase::parse(query, &params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Parameter $5"));
    }

    #[test]
    fn parse_delete_valid() {
        let query = "DELETE FROM table WHERE PartitionKey = $1 AND RowKey = $2";
        let params =
            vec![DataType::Str(Some("part1".to_string())), DataType::Str(Some("row1".to_string()))];

        let result = ExecPhrase::parse(query, &params).unwrap();

        assert_eq!(result.partition, "part1");
        assert_eq!(result.row, "row1");
        assert_eq!(result.entity.len(), 0); // DELETE doesn't have entity fields
    }

    #[test]
    fn parse_delete_missing_where_clause() {
        let query = "DELETE FROM table";
        let params = vec![];

        let result = ExecPhrase::parse(query, &params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("WHERE clause"));
    }

    #[test]
    fn parse_delete_empty_where_clause() {
        let query = "DELETE FROM table WHERE ";
        let params = vec![];

        let result = ExecPhrase::parse(query, &params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("WHERE clause cannot be empty"));
    }

    #[test]
    fn parse_delete_missing_partition_key_in_where() {
        let query = "DELETE FROM table WHERE RowKey = $1";
        let params = vec![DataType::Str(Some("row1".to_string()))];

        let result = ExecPhrase::parse(query, &params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("PartitionKey"));
    }

    #[test]
    fn parse_delete_missing_row_key_in_where() {
        let query = "DELETE FROM table WHERE PartitionKey = $1";
        let params = vec![DataType::Str(Some("part1".to_string()))];

        let result = ExecPhrase::parse(query, &params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("RowKey"));
    }

    #[test]
    fn parse_delete_extra_where_conditions() {
        let query = "DELETE FROM table WHERE PartitionKey = $1 AND RowKey = $2 AND Age = $3";
        let params = vec![
            DataType::Str(Some("part1".to_string())),
            DataType::Str(Some("row1".to_string())),
            DataType::Int32(Some(30)),
        ];

        let result = ExecPhrase::parse(query, &params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unsupported conditions"));
    }

    #[test]
    fn parse_delete_out_of_bounds_param() {
        let query = "DELETE FROM table WHERE PartitionKey = $5 AND RowKey = $2";
        let params = vec![DataType::Str(Some("row1".to_string()))];

        let result = ExecPhrase::parse(query, &params);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Parameter $5"));
    }
}
