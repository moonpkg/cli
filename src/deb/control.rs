#[derive(Default)]
pub struct DebMeta {
    fields: Vec<(String, String)>,
}

impl DebMeta {
    pub fn get(&self, k: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(a, _)| a.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.trim())
            .filter(|v| !v.is_empty())
    }
}

pub fn parse_control(text: &str) -> DebMeta {
    let mut m = DebMeta::default();
    let mut last: Option<usize> = None;
    for line in text.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(i) = last {
                let add = format!("\n{}", line.trim());
                m.fields[i].1.push_str(&add);
            }
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once(':') {
            m.fields.push((k.trim().to_string(), v.trim().to_string()));
            last = Some(m.fields.len() - 1);
        } else {
            last = None;
        }
    }
    m
}
