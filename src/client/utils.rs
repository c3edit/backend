use loro::LoroDoc;

pub fn generate_unique_id(name: &str, doc: &mut LoroDoc) -> String {
    let mut i = 0;
    let mut unique_name = name.to_string();

    while !doc.get_text(unique_name.as_str()).is_empty() {
        i += 1;
        unique_name = format!("{} {}", name, i);
    }

    unique_name
}
