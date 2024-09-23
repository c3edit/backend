use loro::{event::ContainerDiff, LoroDoc, TextDelta};

use super::Change;

pub fn generate_unique_id(name: &str, doc: &mut LoroDoc) -> String {
    let mut i = 0;
    let mut unique_name = name.to_string();

    while !doc.get_text(unique_name.as_str()).is_empty() {
        i += 1;
        unique_name = format!("{}-{}", name, i);
    }

    unique_name
}

pub fn diffs_to_changes(c_diffs: &[ContainerDiff]) -> Vec<Change> {
    let mut changes = Vec::new();

    for c_diff in c_diffs {
        let deltas = c_diff.diff.as_text().unwrap();
        let mut index = 0;

        for diff in deltas {
            match diff {
                TextDelta::Retain { retain, .. } => {
                    index += retain;
                }
                TextDelta::Insert { insert, .. } => {
                    changes.push(Change::Insert {
                        index,
                        text: insert.to_string(),
                    });
                }
                TextDelta::Delete { delete, .. } => {
                    changes.push(Change::Delete {
                        index,
                        len: *delete,
                    });
                }
            }
        }
    }

    changes
}
