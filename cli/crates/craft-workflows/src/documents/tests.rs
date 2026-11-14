use super::*;

#[test]
#[ignore = "Requires an explicitly fetched Atlassian ADF schema fixture; run separately from offline gates"]
fn rendered_documents_validate_against_the_fetched_atlassian_schema()
-> Result<(), Box<dyn std::error::Error>> {
    use std::io::Read;
    let path = std::env::var_os("CRAFT_ADF_SCHEMA")
        .ok_or("Set CRAFT_ADF_SCHEMA to the fetched vendor JSON schema")?;
    let mut schema = Vec::new();
    std::fs::File::open(path)?
        .take(1_048_577)
        .read_to_end(&mut schema)?;
    if schema.len() > 1_048_576 {
        return Err("Vendor schema fixture exceeds its bound".into());
    }
    assert_eq!(
        craft_core::input::content_digest(&schema),
        "sha256:5128562b75278c8a83e7e3619a570205bc80d59696985ec31a7a7883cff66fbe",
        "Vendor schema fixture differs from the qualified Atlassian version"
    );
    let schema: Value = serde_json::from_slice(&schema)?;
    let validator = jsonschema::validator_for(&schema)?;
    let plan = crate::plans::tests::plan()?;
    let mut design = crate::plans::Design::scaffold(
        "invoice-design".into(),
        "Invoice design".into(),
        crate::plans::DesignKind::Hld,
        plan.target.confluence.clone(),
        true,
    )?;
    for section in &mut design.sections {
        section.body = crate::plans::RichText::Markdown(format!("Explain {}.", section.title));
    }
    for document in [plan.render()?, design.render()?] {
        let value = document.to_adf()?;
        assert!(
            validator.is_valid(&value),
            "Composed planning page does not match the vendor schema"
        );
    }
    for source in [
        "",
        "# Heading\n\n**Bold** _em_ `code` [link](https://example.com)  \nnext\n\n---\n",
        "- first\n  - nested\n- second\n\n3. ordered\n\n> quote\n",
        "```rust\n日本語 ]]>\n```\n",
        "| Field | Value |\n| --- | --- |\n| Title | Human title |\n",
    ] {
        let document = Document::from_markdown(source)?;
        let value = document.to_adf()?;
        let errors: Vec<_> = validator
            .iter_errors(&value)
            .map(|error| format!("{}: {error}", error.instance_path()))
            .collect();
        assert!(errors.is_empty(), "{}", errors.join("\n"));
    }
    for source in [
        "- parent\n\n  > nested quote\n",
        "- parent\n\n  # nested heading\n",
        "> | Field | Value |\n> | --- | --- |\n> | Title | Nested |\n",
    ] {
        assert_eq!(
            Document::from_markdown(source)
                .err()
                .ok_or("Accepted unsupported ADF nesting")?
                .reason,
            "unsupported_structure"
        );
    }
    Ok(())
}

#[test]
fn supported_documents_round_trip_through_storage_and_adf() -> Result<(), Box<dyn std::error::Error>>
{
    let inline_list =
        Document::from_storage("<ul><li><strong>one</strong> <em>two</em></li></ul>")?;
    assert!(
        inline_list
            .to_storage()?
            .contains("<strong>one</strong> <em>two</em>")
    );
    for source in [
        "",
        "# 日本語 🦀\n\nA **bold _word_** and [link](https://example.com/?a=1&b=2).  \nnext\n\n---\n",
        "- first\n  - nested\n- second\n\n3. numbered\n\n> quoted\n",
        "```rust\n日本語\n</ac:plain-text-body> & ]]>\n```\n\n`x < y`\n",
        "| Field | Value |\n| --- | --- |\n| Title | **Human edit** |\n| Empty | |\n| Example | `a\\|b` |\n",
        "&lt;unsafe&gt; &amp; \"quoted\"\n\n    indented code\n",
    ] {
        let original = Document::from_markdown(source)?;
        let expected = original.to_adf()?;
        let storage = original.to_storage()?;
        assert_eq!(Document::from_storage(&storage)?.to_adf()?, expected);
        assert_eq!(
            Document::from_adf(&serde_json::to_vec(&expected)?)?.to_adf()?,
            expected
        );
    }
    Ok(())
}

#[test]
fn visible_human_edits_replace_original_values_and_keep_rich_content()
-> Result<(), Box<dyn std::error::Error>> {
    let original = Document::from_markdown(
        "# Delivery plan\n\n## Work item\n\n| Field | Value |\n| --- | --- |\n| Title | Original title |\n| Reference | [Design](https://example.com/design) |\n\n### Description\nOriginal acceptance.\n",
    )?;
    let edited = original
        .to_storage()?
        .replace("Original title", "Human title &amp; revision")
        .replace("Original acceptance.", "Human acceptance 🦀.");
    let imported = Document::from_storage(&edited)?;
    let sections = imported.sections()?;
    assert_eq!(sections.len(), 3);
    assert_eq!(sections[1].level, 2);
    let fields = sections[1].body.field_table()?;
    assert_eq!(fields[0].name, "Title");
    assert_eq!(fields[0].value.plain_text()?, "Human title & revision");
    assert!(fields[1].value.plain_text().is_err());
    assert_eq!(
        fields[1].value.to_adf()?["content"][0]["content"][0]["marks"][0]["attrs"]["href"],
        "https://example.com/design"
    );
    assert_eq!(sections[2].body.plain_text()?, "Human acceptance 🦀.");
    let adf = serde_json::to_vec(&imported.to_adf()?)?;
    let from_adf = Document::from_adf(&adf)?.sections()?;
    assert_eq!(
        from_adf[1].body.field_table()?[0].value.plain_text()?,
        "Human title & revision"
    );
    assert_eq!(from_adf[1].source_path.as_deref(), Some("/content/1"));
    Ok(())
}

#[test]
fn storage_import_rejects_macros_declarations_attributes_and_ambiguous_tables() {
    for source in [
        "<p>safe</p><ac:structured-macro ac:name=\"include\"><ac:plain-text-body>secret-source</ac:plain-text-body></ac:structured-macro>",
        "<!DOCTYPE page SYSTEM \"file:///secret-source\"><p>safe</p>",
        "<p>&secret-source;</p>",
        "<p style=\"display:none\">secret-source</p>",
        "<p><script>secret-source</script></p>",
        "<p><b>secret-source</p>",
        "<table><tr><td colspan=\"2\">secret-source</td></tr></table>",
        "<table><tr><th>one</th><td>secret-source</td></tr></table>",
        "<table><tr><td><p>one</p><p>secret-source</p></td></tr></table>",
        "<ac:structured-macro ac:name=\"code\"><ac:parameter ac:name=\"title\">secret-source</ac:parameter><ac:plain-text-body>x</ac:plain-text-body></ac:structured-macro>",
        "<a href=\"https://example.com\" href=\"secret-source\">text</a>",
        "<p><strong><code>secret-source</code></strong></p>",
        "<ul/>",
    ] {
        let error = Document::from_storage(source)
            .err()
            .unwrap_or_else(|| panic!("Accepted unsupported storage"));
        assert!(error.offset < source.len());
        assert!(!format!("{error:?} {error}").contains("secret-source"));
    }
}

#[test]
fn adf_import_reports_safe_paths_for_unsupported_or_invalid_content()
-> Result<(), Box<dyn std::error::Error>> {
    for node in [
        json!({"type":"extension","attrs":{"name":"secret-source"}}),
        json!({"type":"paragraph","secret-source":true}),
        json!({"type":"paragraph","content":[{"type":"text","text":"secret-source","marks":[{"type":"unsupported"}]}]}),
        json!({"type":"paragraph","content":[{"type":"text","text":"secret-source","marks":[{"type":"code"},{"type":"strong"}]}]}),
        json!({"type":"codeBlock","content":[{"type":"text","text":"secret-source","marks":[{"type":"strong"}]}]}),
        json!({"type":"paragraph","content":[{"type":"text","text":"\u{0}secret-source"}]}),
        json!({"type":"tableCell","content":[{"type":"paragraph"}]}),
    ] {
        let bytes = serde_json::to_vec(&json!({"type":"doc","version":1,"content":[node]}))?;
        let error = Document::from_adf(&bytes)
            .err()
            .ok_or("Accepted invalid ADF")?;
        assert!(error.path.is_some());
        assert!(!format!("{error:?} {error}").contains("secret-source"));
    }
    assert_eq!(
        Document::from_adf(br#"{"type":"doc","type":"secret-source","version":1}"#)
            .err()
            .ok_or("Accepted duplicate key")?
            .reason,
        "invalid_adf_json"
    );
    Ok(())
}

#[test]
fn raw_retrieval_is_explicit_and_ambiguous_field_tables_are_refused()
-> Result<(), Box<dyn std::error::Error>> {
    let source = "<ac:structured-macro ac:name=\"include\"/>".to_owned();
    let raw = RawDocument::new(DocumentFormat::Storage, source.clone())?;
    assert_eq!(raw.format(), DocumentFormat::Storage);
    assert_eq!(raw.raw(), source);
    assert!(raw.import().is_err());
    for source in [
        "| Field | Value |\n| --- | --- |\n| Title | one |\n| title | two |\n",
        "| Name | Value |\n| --- | --- |\n| Title | one |\n",
        "A human note.\n\n| Field | Value |\n| --- | --- |\n| Title | one |\n",
    ] {
        assert!(Document::from_markdown(source)?.field_table().is_err());
    }
    assert!(
        Document::from_markdown("Preamble\n\n# Heading")?
            .sections()
            .is_err()
    );
    Ok(())
}

#[test]
fn imported_content_observes_input_text_event_and_depth_limits()
-> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        Document::from_storage("<p>safe\0invalid</p>")
            .err()
            .ok_or("Accepted invalid XML character")?
            .offset,
        7
    );
    let nested = (0..24)
        .map(|depth| format!("{}- nested\n", "  ".repeat(depth)))
        .collect::<String>();
    assert_eq!(
        Document::from_markdown(&nested)?
            .to_adf()
            .err()
            .ok_or("Missing physical JSON bound")?
            .reason,
        "adf_structure_limit"
    );
    assert_eq!(
        Document::from_storage(&format!("<p>{}</p>", "x".repeat(MAX_INPUT_BYTES + 1)))
            .err()
            .ok_or("Missing text bound")?
            .reason,
        "text_byte_limit"
    );
    assert_eq!(
        Document::from_storage(&"<p/>".repeat(MAX_EVENTS + 1))
            .err()
            .ok_or("Missing event bound")?
            .reason,
        "event_limit"
    );
    assert_eq!(
        Document::from_storage(&format!(
            "{}text{}",
            "<blockquote>".repeat(70),
            "</blockquote>".repeat(70)
        ))
        .err()
        .ok_or("Missing nesting bound")?
        .reason,
        "depth_limit"
    );
    assert!(RawDocument::new(DocumentFormat::Adf, "x".repeat(MAX_OUTPUT_BYTES + 1)).is_err());
    Ok(())
}

#[test]
fn narrative_marks_links_lists_and_breaks_match_adf_and_storage()
-> Result<(), Box<dyn std::error::Error>> {
    let document = Document::from_markdown(
        "# 日本語 🦀\n\nA **bold _word_** and [link](https://example.com/?a=1&b=2).\nwrap  \nnext\n\n3. first\n4. second\n\n> quote\n\n---\n",
    )?;
    let adf = document.to_adf()?;
    assert_eq!(adf["version"], 1);
    assert_eq!(
        adf["content"][0],
        json!({"type":"heading","attrs":{"level":1},"content":[{"type":"text","text":"日本語 🦀"}]})
    );
    let paragraph = &adf["content"][1]["content"];
    assert_eq!(
        paragraph[2],
        json!({"type":"text","text":"word","marks":[{"type":"strong"},{"type":"em"}]})
    );
    assert_eq!(adf["content"][2]["type"], "orderedList");
    assert_eq!(adf["content"][2]["attrs"]["order"], 3);
    assert_eq!(
        adf["content"][2]["content"][0]["content"][0]["type"],
        "paragraph"
    );
    let storage = document.to_storage()?;
    assert!(storage.contains("<h1>日本語 🦀</h1>"));
    assert!(storage.contains("<strong>bold <em>word</em></strong>"));
    assert!(storage.contains("href=\"https://example.com/?a=1&amp;b=2\""));
    assert!(storage.contains("wrap<br />next"));
    assert!(storage.contains("<ol start=\"3\"><li><p>first</p></li>"));
    assert!(storage.ends_with("<blockquote><p>quote</p></blockquote><hr />"));
    assert_eq!(document.to_storage()?, storage);
    assert_eq!(document.to_adf()?, adf);
    Ok(())
}

#[test]
fn code_round_trips_through_xml_cdata_without_injecting_markup()
-> Result<(), Box<dyn std::error::Error>> {
    let code = "日本語🦀\n</ac:plain-text-body> & <script>]]>\n";
    let document = Document::from_markdown(&format!("```rust\n{code}```\n"))?;
    assert_eq!(
        document.to_adf()?["content"][0],
        json!({"type":"codeBlock","attrs":{"language":"rust"},"content":[{"type":"text","text":code}]})
    );
    let storage = document.to_storage()?;
    assert!(storage.contains("]]]]><![CDATA[>"));
    let mut reader = quick_xml::Reader::from_str(&storage);
    let mut actual = String::new();
    let mut depth = 0usize;
    let mut macros = 0;
    loop {
        match reader.read_event()? {
            quick_xml::events::Event::Start(start) => {
                depth += 1;
                if start.name().as_ref() == b"ac:structured-macro" {
                    macros += 1;
                }
            }
            quick_xml::events::Event::End(_) => {
                depth -= 1;
            }
            quick_xml::events::Event::CData(data) => actual.push_str(std::str::from_utf8(&data)?),
            quick_xml::events::Event::Eof => break,
            _ => {}
        }
    }
    assert_eq!(depth, 0);
    assert_eq!(macros, 1);
    assert_eq!(actual, code);
    Ok(())
}

#[test]
fn tables_preserve_inline_content_and_refuse_padding_or_dropped_cells()
-> Result<(), Box<dyn std::error::Error>> {
    let document = Document::from_markdown(
        "| Field | Value |\n| --- | --- |\n| Title | **日本語** |\n| Example | `a\\|b` |\n",
    )?;
    let adf = document.to_adf()?;
    assert_eq!(adf["content"][0]["type"], "table");
    assert_eq!(
        adf["content"][0]["content"][0]["content"][0]["type"],
        "tableHeader"
    );
    assert_eq!(
        adf["content"][0]["content"][2]["content"][1]["content"][0]["content"][0]["text"],
        "a|b"
    );
    assert!(
        document
            .to_storage()?
            .contains("<th>Field</th><th>Value</th>")
    );
    for row in ["| a |", "| a | b | lost |"] {
        let source = format!("| Field | Value |\n| --- | --- |\n{row}\n");
        let error = Document::from_markdown(&source)
            .err()
            .ok_or("Accepted non-rectangular table")?;
        assert_eq!(error.reason, "non_rectangular_table");
        assert_eq!(&source[error.offset..], format!("{row}\n"));
    }
    Ok(())
}

#[test]
fn unsupported_constructs_fail_at_source_offsets_without_echoing_content() {
    for source in [
        "hello <script>secret-source</script>",
        "![secret-source](https://example.com/image)",
        "- [ ] secret-source",
        "[secret-source](javascript:alert)",
        "[secret-source](https://user:password@example.com)",
        "**`secret-source`**",
        "- parent\n\n  > secret-source\n",
        "> # secret-source\n",
        "> > secret-source\n",
        "~~secret-source~~",
        "# secret-source {#custom}",
        "[^secret-source]\n\n[^secret-source]: hidden",
        "a\0secret-source",
    ] {
        let error = Document::from_markdown(source)
            .err()
            .unwrap_or_else(|| panic!("Accepted unsupported content"));
        assert!(error.offset < source.len());
        assert!(!format!("{error:?} {error}").contains("secret-source"));
    }
}

#[test]
fn bounds_and_duplicate_marks_are_enforced() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(
        Document::from_markdown(&"x".repeat(MAX_INPUT_BYTES + 1))
            .err()
            .ok_or("Missing input bound")?
            .reason,
        "input_byte_limit"
    );
    let deep = format!("{}text\n", "> ".repeat(70));
    assert_eq!(
        Document::from_markdown(&deep)
            .err()
            .ok_or("Missing depth bound")?
            .reason,
        "depth_limit"
    );
    assert_eq!(
        Document::from_markdown(&"a  \nb  \n".repeat(20_000))
            .err()
            .ok_or("Missing event bound")?
            .reason,
        "event_limit"
    );
    let adf = Document::from_markdown("****word****")?.to_adf()?;
    assert_eq!(
        adf["content"][0]["content"][0]["marks"],
        json!([{"type":"strong"}])
    );
    assert_eq!(
        Document::from_markdown("")?.to_adf()?,
        json!({"type":"doc","version":1,"content":[{"type":"paragraph","content":[]}]})
    );
    Ok(())
}
