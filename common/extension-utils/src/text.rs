use scraper::ElementRef;

/// Trim only the outer whitespace; preserve spacing and line breaks inside text.
pub fn optional_text(text: String) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

pub fn element_text(element: ElementRef<'_>) -> Option<String> {
    optional_text(element.text().collect())
}

/// Keep the first occurrence of each nonempty name without changing its case.
pub fn unique_text(values: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut result = Vec::new();
    for value in values.into_iter().filter_map(optional_text) {
        if !result.contains(&value) {
            result.push(value);
        }
    }
    result
}

/// Extract a description as plain text with paragraph and explicit line breaks.
pub fn description_text(element: ElementRef<'_>) -> Option<String> {
    fn paragraph_break(output: &mut String) {
        output.truncate(output.trim_end().len());
        if !output.is_empty() {
            output.push_str("\n\n");
        }
    }

    fn append(element: ElementRef<'_>, output: &mut String) {
        let name = element.value().name();
        if matches!(name, "script" | "style" | "template") {
            return;
        }
        if name == "br" {
            output.push('\n');
            return;
        }
        let block = matches!(
            name,
            "p" | "div"
                | "section"
                | "article"
                | "blockquote"
                | "pre"
                | "li"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "hr"
        );
        if block {
            paragraph_break(output);
        }
        for child in element.children() {
            if let Some(text) = child.value().as_text() {
                output.push_str(text);
            } else if let Some(child) = ElementRef::wrap(child) {
                append(child, output);
            }
        }
        if block {
            paragraph_break(output);
        }
    }

    let mut output = String::new();
    append(element, &mut output);
    optional_text(output)
}

#[cfg(test)]
mod test {
    use super::*;
    use scraper::{Html, Selector};

    #[test]
    fn nested_metadata_preserves_names_spacing_and_paragraphs() {
        let document = Html::parse_fragment(
            "<div><p>First  <b>paragraph &amp; text</b>.</p><p>Second<br>line.</p></div><a> Jane <b>Doe</b> </a>",
        );
        let description = document
            .select(&Selector::parse("div").unwrap())
            .next()
            .unwrap();
        let author = document
            .select(&Selector::parse("a").unwrap())
            .next()
            .unwrap();
        assert_eq!(
            description_text(description).as_deref(),
            Some("First  paragraph & text.\n\nSecond\nline.")
        );
        assert_eq!(element_text(author).as_deref(), Some("Jane Doe"));
        assert_eq!(
            unique_text([
                " Jane Doe ".into(),
                "".into(),
                "Jane Doe".into(),
                "Another".into()
            ]),
            ["Jane Doe", "Another"]
        );
        assert_eq!(optional_text(" \n ".into()), None);
    }
}
