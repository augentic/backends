//! Functions and types to support modifier queries against Azure Table Storage.
//! Only INSERT, UPDATE, and DELETE are supported.

use anyhow::{anyhow, bail};
use qwasr_wasi_sql::{DataType, Field};

use super::sql_to_odata_filter;

#[derive(Debug)]
pub enum ExecAction {
    Insert,
    Update,
    Delete,
}

#[derive(Debug)]
pub struct ExecPhrase {
    pub action: ExecAction,
    pub filter: Option<String>,
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
        
        // Extract WHERE clause if present
        let filter = Self::extract_where_clause(query);
        
        // Parse fields based on action, using original parameter types
        let entity = match action {
            ExecAction::Insert => Self::parse_insert(query, params)?,
            ExecAction::Update => Self::parse_update(query, params)?,
            ExecAction::Delete => Self::parse_delete(),
        };
        
        Ok(Self { action, filter, entity })
    }
    
    fn parse_insert(query: &str, params: &[DataType]) -> anyhow::Result<Vec<Field>> {
        // Parse INSERT INTO table (col1, col2) VALUES ($1, $2)
        let query_upper = query.to_uppercase();
        
        // Find VALUES keyword
        let values_pos = query_upper.find("VALUES")
            .ok_or_else(|| anyhow!("INSERT query must contain VALUES clause"))?;
        
        // Extract column names from INSERT clause
        let insert_part = &query[..values_pos];
        let columns = Self::extract_columns(insert_part)?;
        
        // Extract parameter placeholders from VALUES clause
        let values_part = &query[values_pos + 6..]; // Skip "VALUES"
        let param_indices = Self::extract_param_placeholders(values_part)?;
        
        // Check that columns and values match
        if columns.len() != param_indices.len() {
            bail!("number of columns ({}) does not match number of values ({})", 
                  columns.len(), param_indices.len());
        }
        
        // Create Field entries using original parameter types
        let mut fields = Vec::new();
        for (col, param_idx) in columns.iter().zip(param_indices.iter()) {
            if *param_idx >= params.len() {
                bail!("parameter ${} referenced but only {} parameters provided", 
                      param_idx + 1, params.len());
            }
            fields.push(Field {
                name: col.clone(),
                value: params[*param_idx].clone(),
            });
        }
        
        Ok(fields)
    }
    
    fn parse_update(query: &str, params: &[DataType]) -> anyhow::Result<Vec<Field>> {
        // Parse UPDATE table SET col1 = $1, col2 = $2 WHERE ...
        let query_upper = query.to_uppercase();
        
        // Find SET keyword
        let set_pos = query_upper.find(" SET ")
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
                    bail!("Parameter ${} referenced but only {} parameters provided", 
                          param_idx + 1, params.len());
                }
                fields.push(Field {
                    name: col_name,
                    value: params[param_idx].clone(),
                });
            } else {
                bail!("UPDATE SET clause must use parameter placeholders (e.g., $1, $2)");
            }
        }
        
        Ok(fields)
    }
    
    const fn parse_delete() -> Vec<Field> {
        // DELETE queries don't need entity fields
        // The WHERE clause will be handled separately if needed
        Vec::new()
    }
    
    fn extract_columns(insert_part: &str) -> anyhow::Result<Vec<String>> {
        // Find the opening parenthesis for column names
        let open_paren = insert_part.find('(')
            .ok_or_else(|| anyhow!("INSERT query must specify column names in parentheses"))?;
        let close_paren = insert_part.rfind(')')
            .ok_or_else(|| anyhow!("INSERT query must specify column names in parentheses"))?;
        
        let columns_str = &insert_part[open_paren + 1..close_paren];
        let columns: Vec<String> = columns_str
            .split(',')
            .map(|s| s.trim().to_string())
            .collect();
        
        Ok(columns)
    }
    
    fn extract_param_placeholders(values_part: &str) -> anyhow::Result<Vec<usize>> {
        // Find the opening and closing parentheses for values
        let open_paren = values_part.find('(')
            .ok_or_else(|| anyhow!("VALUES clause must contain values in parentheses"))?;
        let close_paren = values_part.rfind(')')
            .ok_or_else(|| anyhow!("VALUES clause must contain values in parentheses"))?;
        
        let values_str = &values_part[open_paren + 1..close_paren];
        
        // Split by comma and extract parameter indices
        let mut param_indices = Vec::new();
        for value in values_str.split(',') {
            let trimmed = value.trim();
            if let Some(idx) = Self::parse_param_placeholder(trimmed) {
                param_indices.push(idx);
            } else {
                bail!("VALUES clause must use parameter placeholders (e.g., $1, $2), found: {trimmed}");
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
    
    fn extract_where_clause(query: &str) -> Option<String> {
        let query_upper = query.to_uppercase();
        
        // Find WHERE keyword and extract everything after it
        query_upper.find(" WHERE ").and_then(|where_pos| {
            let where_clause = query[where_pos + 7..].trim();
            
            if where_clause.is_empty() {
                None
            } else {
                // Convert SQL operators to OData operators
                Some(sql_to_odata_filter(where_clause))
            }
        })
    }
}