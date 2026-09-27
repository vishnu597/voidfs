// SPDX-License-Identifier: Apache-2.0
//! Variables, JSON and XML paths, and matchers (README "Matchers", "Paths", "Variables").

use std::collections::HashMap;

use serde_json::Value;

use crate::cases::Matcher;

pub type Vars = HashMap<String, String>;

/// Replaces `${name}` with its value; `$${` is a literal `${`.
pub fn subst(s: &str, vars: &Vars) -> Result<String, String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(after) = tail.strip_prefix("$${") {
            out.push_str("${");
            rest = after;
        } else if let Some(body) = tail.strip_prefix("${") {
            let end = body.find('}').ok_or_else(|| format!("unterminated variable in {s:?}"))?;
            let name = &body[..end];
            out.push_str(vars.get(name).ok_or_else(|| format!("variable ${{{name}}} is not set"))?);
            rest = &body[end + 1..];
        } else {
            out.push('$');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    Ok(out)
}

fn subst_value(v: &Value, vars: &Vars) -> Result<Value, String> {
    Ok(match v {
        Value::String(s) => Value::String(subst(s, vars)?),
        Value::Array(a) => Value::Array(a.iter().map(|x| subst_value(x, vars)).collect::<Result<_, _>>()?),
        other => other.clone(),
    })
}

/// Evaluates a JSON path. `None` when it leads nowhere.
pub fn json_path(root: &Value, path: &str) -> Result<Option<Value>, String> {
    let mut rest = path.strip_prefix('$').ok_or_else(|| format!("JSON path {path:?} must start with $"))?;
    let mut cur: Option<Value> = Some(root.clone());
    while !rest.is_empty() {
        let Some(v) = cur.take() else { return Ok(None) };
        if let Some(r) = rest.strip_prefix("['") {
            let end = r.find("']").ok_or_else(|| format!("bad JSON path {path:?}"))?;
            cur = v.get(&r[..end]).cloned();
            rest = &r[end + 2..];
        } else if let Some(r) = rest.strip_prefix('[') {
            let end = r.find(']').ok_or_else(|| format!("bad JSON path {path:?}"))?;
            let i: i64 = r[..end].parse().map_err(|_| format!("bad index in {path:?}"))?;
            cur = v.as_array().and_then(|a| {
                let idx = if i < 0 { a.len() as i64 + i } else { i };
                usize::try_from(idx).ok().and_then(|j| a.get(j)).cloned()
            });
            rest = &r[end + 1..];
        } else if let Some(r) = rest.strip_prefix('.') {
            let end = r.find(['.', '[']).unwrap_or(r.len());
            let name = &r[..end];
            cur = match (&v, name) {
                (Value::Array(a), "length") => Some(Value::from(a.len())),
                (Value::String(s), "length") => Some(Value::from(s.chars().count())),
                _ => v.get(name).cloned(),
            };
            rest = &r[end..];
        } else {
            return Err(format!("bad JSON path {path:?}"));
        }
    }
    Ok(cur)
}

/// Every element matching an XML path, as text, in document order.
pub fn xml_path(doc: &roxmltree::Document<'_>, path: &str) -> Vec<String> {
    let parts: Vec<&str> = path.split('/').collect();
    let root = doc.root_element();
    if root.tag_name().name() != parts[0] {
        return Vec::new();
    }
    let mut nodes = vec![root];
    for part in &parts[1..] {
        nodes = nodes
            .iter()
            .flat_map(|n| n.children().filter(|c| c.is_element() && c.tag_name().name() == *part))
            .collect();
    }
    nodes.iter().map(|n| n.descendants().filter(|d| d.is_text()).filter_map(|d| d.text()).collect::<String>()).collect()
}

fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn regex(pattern: &str) -> Result<regex::Regex, String> {
    regex::Regex::new(&format!("^(?:{pattern})$")).map_err(|e| e.to_string())
}

/// What a matcher is applied to.
pub enum Subject<'a> {
    /// A header or a JSON value; `None` when absent.
    Value(Option<Value>),
    /// Every match of an XML path.
    Xml(&'a [String]),
}

/// Checks one matcher; stores captures in `vars`. Returns a description of the failure.
pub fn check(m: &Matcher, subject: Subject<'_>, vars: &mut Vars) -> Result<(), String> {
    match subject {
        Subject::Value(v) => {
            if let Some(p) = m.present
                && v.is_some() != p {
                    return Err(if p { "is absent".into() } else { format!("is present ({})", v.map(|x| as_text(&x)).unwrap_or_default()) });
                }
            let Some(v) = v else {
                return if m.present == Some(false) { Ok(()) } else { Err("is absent".into()) };
            };
            if let Some(e) = &m.equals {
                let e = subst_value(e, vars)?;
                let same = match (&e, &v) {
                    (Value::String(a), b) if !b.is_string() => *a == as_text(b),
                    _ => e == v,
                };
                if !same {
                    return Err(format!("is {} but should equal {}", as_text(&v), as_text(&e)));
                }
            }
            if let Some(ne) = &m.not_equals {
                let ne = subst_value(ne, vars)?;
                if as_text(&ne) == as_text(&v) {
                    return Err(format!("should not equal {}", as_text(&ne)));
                }
            }
            if let Some(p) = &m.matches
                && !regex(p)?.is_match(&as_text(&v)) {
                    return Err(format!("is {} and does not match /{p}/", as_text(&v)));
                }
            if let Some(c) = &m.contains {
                let c = subst_value(c, vars)?;
                let found = v.as_array().is_some_and(|a| a.iter().any(|x| as_text(x) == as_text(&c)));
                if !found {
                    return Err(format!("is {} and does not contain {}", as_text(&v), as_text(&c)));
                }
            }
            if let Some(n) = m.count {
                let len = v.as_array().map(Vec::len).ok_or("is not an array")?;
                if len != n {
                    return Err(format!("has {len} elements, expected {n}"));
                }
            }
            if let Some(name) = &m.capture {
                vars.insert(name.clone(), as_text(&v));
            }
        }
        Subject::Xml(found) => {
            if let Some(p) = m.present
                && found.is_empty() == p {
                    return Err(if p { "is absent".into() } else { "is present".into() });
                }
            if let Some(e) = &m.equals {
                let want: Vec<String> = match subst_value(e, vars)? {
                    Value::Array(a) => a.iter().map(as_text).collect(),
                    other => vec![as_text(&other)],
                };
                if found != want {
                    return Err(format!("is {found:?} but should equal {want:?}"));
                }
            }
            if let Some(c) = &m.contains {
                let c = as_text(&subst_value(c, vars)?);
                if !found.contains(&c) {
                    return Err(format!("is {found:?} and does not contain {c:?}"));
                }
            }
            if let Some(n) = m.count
                && found.len() != n {
                    return Err(format!("has {} matches, expected {n}", found.len()));
                }
            if let Some(p) = &m.matches {
                let re = regex(p)?;
                if found.is_empty() || !found.iter().all(|f| re.is_match(f)) {
                    return Err(format!("is {found:?} and does not match /{p}/"));
                }
            }
            if let Some(ne) = &m.not_equals {
                let ne = as_text(&subst_value(ne, vars)?);
                if found.first() == Some(&ne) {
                    return Err(format!("should not equal {ne:?}"));
                }
            }
            if let Some(name) = &m.capture {
                let first = found.first().ok_or("is absent, nothing to capture")?;
                vars.insert(name.clone(), first.clone());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn substitution() {
        let vars: Vars = [("drive".to_string(), "vfc-1".to_string())].into();
        assert_eq!(subst("/${drive}/a", &vars).unwrap(), "/vfc-1/a");
        assert_eq!(subst("$${drive} costs $5", &vars).unwrap(), "${drive} costs $5");
        assert!(subst("${nope}", &vars).is_err());
    }

    #[test]
    fn json_paths() {
        let v = json!({"versions": [{"id": "a"}, {"id": "b"}], "xattrs": {"user.tag": "aGk="}, "s": "héllo"});
        assert_eq!(json_path(&v, "$.versions.length").unwrap(), Some(json!(2)));
        assert_eq!(json_path(&v, "$.versions[1].id").unwrap(), Some(json!("b")));
        assert_eq!(json_path(&v, "$.versions[-1].id").unwrap(), Some(json!("b")));
        assert_eq!(json_path(&v, "$.xattrs['user.tag']").unwrap(), Some(json!("aGk=")));
        assert_eq!(json_path(&v, "$.s.length").unwrap(), Some(json!(5)));
        assert_eq!(json_path(&v, "$.versions[5].id").unwrap(), None);
        assert_eq!(json_path(&v, "$.missing.deeper").unwrap(), None);
    }

    #[test]
    fn xml_paths_ignore_namespaces() {
        let xml = r#"<ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/"><Contents><Key>a</Key></Contents><Contents><Key>b</Key></Contents></ListBucketResult>"#;
        let doc = roxmltree::Document::parse(xml).unwrap();
        assert_eq!(xml_path(&doc, "ListBucketResult/Contents/Key"), ["a", "b"]);
        assert!(xml_path(&doc, "Other/Contents").is_empty());
    }

    #[test]
    fn matchers() {
        let mut vars = Vars::new();
        let m = |j: Value| serde_json::from_value::<Matcher>(j).unwrap();
        assert!(check(&m(json!({"equals": 2})), Subject::Value(Some(json!(2))), &mut vars).is_ok());
        assert!(check(&m(json!({"equals": "11"})), Subject::Value(Some(json!("11"))), &mut vars).is_ok());
        assert!(check(&m(json!({"equals": false})), Subject::Value(Some(json!(true))), &mut vars).is_err());
        assert!(check(&m(json!({"present": false})), Subject::Value(None), &mut vars).is_ok());
        assert!(check(&m(json!({"matches": "d-[0-9a-f-]{36}"})), Subject::Value(Some(json!("d-0192a7a4-8c1e-7b61-9d3f-3c6a1f0e2b77"))), &mut vars).is_ok());
        assert!(check(&m(json!({"matches": "abc"})), Subject::Value(Some(json!("xabc"))), &mut vars).is_err(), "anchored");
        check(&m(json!({"capture": "v"})), Subject::Value(Some(json!("1.0"))), &mut vars).unwrap();
        assert!(check(&m(json!({"equals": "${v}"})), Subject::Value(Some(json!("1.0"))), &mut vars).is_ok());
        let found = vec!["a".to_string(), "b".to_string()];
        assert!(check(&m(json!({"equals": ["a", "b"], "count": 2, "contains": "b"})), Subject::Xml(&found), &mut vars).is_ok());
        assert!(check(&m(json!({"count": 0})), Subject::Xml(&[]), &mut vars).is_ok());
    }
}
