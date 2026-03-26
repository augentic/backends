//! Translation of [`FilterTree`] into Azure Table `OData` `$filter` strings.
//!
//! Azure Table's `OData` query layer supports comparison operators, logical
//! combinators (`and`, `or`, `not`), and set membership (`InList` / `NotInList`
//! expanded to OR chains). It does **not** support string functions
//! (`contains`, `startswith`, `endswith`) or null-checks — see
//! <https://learn.microsoft.com/en-us/rest/api/storageservices/querying-tables-and-entities#supported-query-options>.
//!
//! Filters that cannot be evaluated server-side are rejected with an error
//! rather than silently falling back to client-side evaluation, which could
//! pull unbounded data from the table service.

use std::fmt::Write;

use anyhow::bail;
use omnia_wasi_jsondb::{ComparisonOp, FilterTree, ScalarValue};

/// Translate a [`FilterTree`] to an `OData` `$filter` string.
///
/// # Errors
///
/// Returns an error if the filter contains nodes that Azure Table cannot
/// evaluate server-side: `IsNull`, `IsNotNull`, `Contains`, `StartsWith`,
/// `EndsWith`, or any logical combinator (`And`, `Or`, `Not`) whose children
/// include such nodes.
pub fn to_odata(filter: &FilterTree) -> anyhow::Result<String> {
    match filter {
        FilterTree::Compare { field, op, value } => {
            Ok(format!("{field} {} {}", odata_op(*op), odata_value(value)))
        }
        FilterTree::InList { field, values } => {
            let parts: Vec<String> =
                values.iter().map(|v| format!("{field} eq {}", odata_value(v))).collect();
            Ok(format!("({})", parts.join(" or ")))
        }
        FilterTree::NotInList { field, values } => {
            let parts: Vec<String> =
                values.iter().map(|v| format!("{field} eq {}", odata_value(v))).collect();
            Ok(format!("not ({})", parts.join(" or ")))
        }
        FilterTree::And(children) => {
            let parts: Vec<String> =
                children.iter().map(to_odata).collect::<anyhow::Result<_>>()?;
            Ok(parts.join(" and "))
        }
        FilterTree::Or(children) => {
            let parts: Vec<String> =
                children.iter().map(to_odata).collect::<anyhow::Result<_>>()?;
            Ok(format!("({})", parts.join(" or ")))
        }
        FilterTree::Not(inner) => Ok(format!("not ({})", to_odata(inner)?)),
        FilterTree::IsNull(field) => {
            bail!(
                "IsNull('{field}') is not supported by Azure Table — properties that are null are omitted from entities entirely"
            )
        }
        FilterTree::IsNotNull(field) => {
            bail!(
                "IsNotNull('{field}') is not supported by Azure Table — properties that are null are omitted from entities entirely"
            )
        }
        FilterTree::Contains { field, .. } => {
            bail!(
                "Contains('{field}') is not supported by Azure Table — OData $filter does not support string functions"
            )
        }
        FilterTree::StartsWith { field, .. } => {
            bail!(
                "StartsWith('{field}') is not supported by Azure Table — OData $filter does not support string functions"
            )
        }
        FilterTree::EndsWith { field, .. } => {
            bail!(
                "EndsWith('{field}') is not supported by Azure Table — OData $filter does not support string functions"
            )
        }
    }
}

const fn odata_op(op: ComparisonOp) -> &'static str {
    match op {
        ComparisonOp::Eq => "eq",
        ComparisonOp::Ne => "ne",
        ComparisonOp::Gt => "gt",
        ComparisonOp::Gte => "ge",
        ComparisonOp::Lt => "lt",
        ComparisonOp::Lte => "le",
    }
}

fn odata_value(v: &ScalarValue) -> String {
    match v {
        ScalarValue::Null => "null".into(),
        ScalarValue::Boolean(b) => b.to_string(),
        ScalarValue::Int32(i) => i.to_string(),
        ScalarValue::Int64(i) => format!("{i}L"),
        ScalarValue::Float64(f) => format!("{f}"),
        ScalarValue::Str(s) => format!("'{}'", s.replace('\'', "''")),
        ScalarValue::Binary(b) => {
            let hex = b.iter().fold(String::with_capacity(b.len() * 2), |mut acc, byte| {
                let _ = write!(acc, "{byte:02x}");
                acc
            });
            format!("X'{hex}'")
        }
        ScalarValue::Timestamp(t) => format!("datetime'{t}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_eq_to_odata() {
        let f = FilterTree::Compare {
            field: "Name".into(),
            op: ComparisonOp::Eq,
            value: ScalarValue::Str("Alice".into()),
        };
        let odata = to_odata(&f).unwrap();
        assert_eq!(odata, "Name eq 'Alice'");
    }

    #[test]
    fn in_list_to_odata() {
        let f = FilterTree::InList {
            field: "Status".into(),
            values: vec![ScalarValue::Int32(1), ScalarValue::Int32(2)],
        };
        let odata = to_odata(&f).unwrap();
        assert_eq!(odata, "(Status eq 1 or Status eq 2)");
    }

    #[test]
    fn is_null_rejected() {
        let f = FilterTree::IsNull("Zone".into());
        to_odata(&f).unwrap_err();
    }

    #[test]
    fn contains_rejected() {
        let f = FilterTree::Contains {
            field: "Name".into(),
            pattern: "Alice".into(),
        };
        to_odata(&f).unwrap_err();
    }

    #[test]
    fn starts_with_rejected() {
        let f = FilterTree::StartsWith {
            field: "Name".into(),
            pattern: "Al".into(),
        };
        to_odata(&f).unwrap_err();
    }

    #[test]
    fn ends_with_rejected() {
        let f = FilterTree::EndsWith {
            field: "Name".into(),
            pattern: "ce".into(),
        };
        to_odata(&f).unwrap_err();
    }

    #[test]
    fn is_not_null_rejected() {
        let f = FilterTree::IsNotNull("Zone".into());
        to_odata(&f).unwrap_err();
    }

    #[test]
    fn and_with_unsupported_child_rejected() {
        let f = FilterTree::And(vec![
            FilterTree::Compare {
                field: "Active".into(),
                op: ComparisonOp::Eq,
                value: ScalarValue::Boolean(true),
            },
            FilterTree::Contains {
                field: "Name".into(),
                pattern: "Alice".into(),
            },
        ]);
        to_odata(&f).unwrap_err();
    }

    #[test]
    fn timestamp_odata() {
        let f = FilterTree::Compare {
            field: "Created".into(),
            op: ComparisonOp::Gte,
            value: ScalarValue::Timestamp("2026-01-01T00:00:00Z".into()),
        };
        let odata = to_odata(&f).unwrap();
        assert_eq!(odata, "Created ge datetime'2026-01-01T00:00:00Z'");
    }

    #[test]
    fn not_in_list_to_odata() {
        let f = FilterTree::NotInList {
            field: "Name".into(),
            values: vec![ScalarValue::Str("Alice".into()), ScalarValue::Str("Bob".into())],
        };
        let odata = to_odata(&f).unwrap();
        assert_eq!(odata, "not (Name eq 'Alice' or Name eq 'Bob')");
    }

    #[test]
    fn not_compare_to_odata() {
        let f = FilterTree::Not(Box::new(FilterTree::Compare {
            field: "IsActive".into(),
            op: ComparisonOp::Eq,
            value: ScalarValue::Boolean(true),
        }));
        let odata = to_odata(&f).unwrap();
        assert_eq!(odata, "not (IsActive eq true)");
    }

    #[test]
    fn or_to_odata() {
        let f = FilterTree::Or(vec![
            FilterTree::Compare {
                field: "Points".into(),
                op: ComparisonOp::Eq,
                value: ScalarValue::Int32(200),
            },
            FilterTree::Compare {
                field: "Points".into(),
                op: ComparisonOp::Eq,
                value: ScalarValue::Int32(150),
            },
        ]);
        let odata = to_odata(&f).unwrap();
        assert_eq!(odata, "(Points eq 200 or Points eq 150)");
    }
}
