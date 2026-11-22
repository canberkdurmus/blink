//! The item list export (FILE-2, CLI-1 `export`). Pure: items and marks in, text out.

use crate::item::Item;
use crate::state::Marks;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Quote a CSV field when it needs it (RFC 4180).
pub fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// One line per member file of each item. `.txt`: the path. `.csv`: a header, then path,
/// rating (empty when unrated), label, and whether the rating is the pick value.
pub fn item_list<'a>(
    items: impl IntoIterator<Item = &'a Item>,
    marks: &BTreeMap<PathBuf, Marks>,
    pick_value: i8,
    csv: bool,
) -> String {
    let mut out = String::new();
    if csv {
        out.push_str("path,rating,label,pick\n");
    }
    for i in items {
        let m = marks.get(&i.primary.path);
        for member in i.members() {
            let p = member.path.to_string_lossy();
            if csv {
                let rating = m
                    .and_then(|m| m.rating)
                    .map(|r| r.to_string())
                    .unwrap_or_default();
                let label = m.and_then(|m| m.label.clone()).unwrap_or_default();
                let pick = m.and_then(|m| m.rating) == Some(pick_value);
                out.push_str(&format!(
                    "{},{rating},{},{pick}\n",
                    csv_field(&p),
                    csv_field(&label)
                ));
            } else {
                out.push_str(&p);
                out.push('\n');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::FileKind;
    use crate::item::Member;

    #[test]
    fn req_file_2_txt_and_csv_lists_every_member_with_its_marks() {
        let items = vec![
            Item {
                primary: Member {
                    path: "/f/a,1.ARW".into(),
                    kind: FileKind::Raw,
                },
                companions: vec![Member {
                    path: "/f/a,1.JPG".into(),
                    kind: FileKind::Jpeg,
                }],
            },
            Item {
                primary: Member {
                    path: "/f/b.ARW".into(),
                    kind: FileKind::Raw,
                },
                companions: vec![],
            },
        ];
        let marks: BTreeMap<_, _> = [(
            PathBuf::from("/f/a,1.ARW"),
            Marks {
                rating: Some(2),
                label: Some("Say \"hi\"".into()),
            },
        )]
        .into();
        assert_eq!(
            item_list(&items, &marks, 2, false),
            "/f/a,1.ARW\n/f/a,1.JPG\n/f/b.ARW\n"
        );
        assert_eq!(
            item_list(&items, &marks, 2, true),
            "path,rating,label,pick\n\
             \"/f/a,1.ARW\",2,\"Say \"\"hi\"\"\",true\n\
             \"/f/a,1.JPG\",2,\"Say \"\"hi\"\"\",true\n\
             /f/b.ARW,,,false\n"
        );
    }
}
