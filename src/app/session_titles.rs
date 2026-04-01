pub fn clean_title_from_text(text: &str) -> Option<String> {
    let mut fallback = None;
    let mut in_instructions = false;
    let mut in_environment = false;

    for raw_line in text.lines() {
        let line = collapse_whitespace(raw_line.trim());
        if line.is_empty() {
            continue;
        }

        match line.as_str() {
            "<INSTRUCTIONS>" => {
                in_instructions = true;
                continue;
            }
            "</INSTRUCTIONS>" => {
                in_instructions = false;
                continue;
            }
            "<environment_context>" => {
                in_environment = true;
                continue;
            }
            "</environment_context>" => {
                in_environment = false;
                continue;
            }
            _ => {}
        }

        if fallback.is_none() {
            fallback = Some(line.clone());
        }

        if !in_instructions && !in_environment && !is_boilerplate_line(&line) {
            return Some(line);
        }
    }

    fallback
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_boilerplate_line(line: &str) -> bool {
    matches!(
        line,
        "<INSTRUCTIONS>"
            | "</INSTRUCTIONS>"
            | "<environment_context>"
            | "</environment_context>"
            | "<attachments>"
            | "</attachments>"
    ) || line.starts_with("# AGENTS.md instructions for ")
        || line.starts_with("<image ")
        || line.starts_with("</image")
        || line.starts_with("<cwd>")
        || line.starts_with("<shell>")
        || line.starts_with("<current_date>")
        || line.starts_with("<timezone>")
        || line.starts_with("```")
}

#[cfg(test)]
mod tests {
    use super::clean_title_from_text;

    #[test]
    fn clean_title_skips_agents_boilerplate() {
        let text = concat!(
            "# AGENTS.md instructions for /tmp/repo\n",
            "\n",
            "<INSTRUCTIONS>\n",
            "stuff\n",
            "</INSTRUCTIONS>\n",
            "<environment_context>\n",
            "  <cwd>/tmp/repo</cwd>\n",
            "  <shell>bash</shell>\n",
            "</environment_context>\n",
            "\n",
            "<image name=[Image #1]>\n",
            "the S key session launcher is really slow to load\n"
        );

        assert_eq!(
            clean_title_from_text(text).as_deref(),
            Some("the S key session launcher is really slow to load")
        );
    }

    #[test]
    fn clean_title_falls_back_to_first_non_empty_line() {
        assert_eq!(
            clean_title_from_text("first line\nsecond line").as_deref(),
            Some("first line")
        );
    }
}
