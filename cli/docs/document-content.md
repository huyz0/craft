# Document content conversion

`craft-workflows::documents::Document` imports a bounded Markdown, storage or ADF subset into a private AST and renders Jira ADF v1 or Confluence storage. It performs no I/O, publication, approval or ticket creation. No public CLI command uses it yet.

## Supported input

Supported content: paragraphs; headings 1 through 6; strong/emphasis; inline code; fenced/indented code blocks; absolute HTTP(S) links without credentials or titles; block quotes; bullet/numbered lists; rules; hard breaks; and rectangular unaligned tables. Tight list items acquire the paragraph structure ADF requires. Duplicate equivalent marks collapse. Soft line wraps normalize to spaces; code retains the CommonMark parser's text and line-ending normalization.

Text and attributes are XML-escaped. Storage code macros split embedded `]]>` across adjacent CDATA sections. ADF code blocks contain unmarked text. Link targets are never fetched.

Raw HTML, images, checkboxes, footnotes, strike, math, heading attributes, metadata and other recognized unsupported constructs return a `ContentError` with a UTF-8 source byte offset and static reason. Link titles, relative/non-HTTP links, credential-bearing URLs, formatted inline code and XML-invalid characters are refused. Extension syntax outside the enabled grammar retains ordinary CommonMark literal meaning.

GFM pads short table rows and discards excess cells. Craft checks recognized source rows and rejects both cases, preserving visible plan fields. Pipe escaping follows GFM, including pipes inside inline code.

List items and block quotes support paragraphs, lists and code. Nested quotes, headings, rules or tables inside those containers are refused where the common ADF representation is invalid.

Limits: 1 MiB Markdown source and decoded AST text/links, 8 MiB raw storage/ADF and serialized output, 65,536 events/nodes and AST depth 64. ADF also obeys the core JSON physical-depth/container limits; output is checked before returning, so some deeply nested Markdown can render storage but receive `adf_structure_limit` for ADF. Empty input renders one empty paragraph. External callers cannot construct or deserialize the AST to bypass checks.

## Import and visible sections

`from_storage` uses a checked XML tree. Known formatting aliases (`b`, `i`, `tt`), table wrappers and single-paragraph cells normalize to the shared model. Only code macros with the supported language parameter import semantically; includes, unknown macros/attributes, merged or mixed-header cells, declarations and unknown entities fail explicitly. No macro is expanded and no DTD or linked content is fetched. Block-separating whitespace is inert; meaningful spaces in inline list text remain. Errors identify source byte offsets.

`from_adf` uses the core duplicate/depth/size-checked JSON parser. Unsupported nodes, marks, attributes and structures fail at a safe JSON path. Malformed JSON is refused without echoing it. Import checks the supported subset rather than accepting every construct in Atlassian's full schema.

`sections()` retains heading level, visible heading, rich body and source origin. It refuses unheaded preamble content rather than dropping a human note. `field_table()` reads exactly one two-column Field/Value table and refuses duplicate names, unexpected headers or extra blocks. Values remain documents. `plain_text()` only accepts an unambiguous text block and refuses links rather than discarding their targets. Plan identities, graph validation and target mappings belong to the plan model.

`RawDocument` requires an explicit format and retains the original bounded string. `raw()` remains usable after `import()` fails. Raw inspection does not claim semantic validity, edit safety or approval authority; there is no implicit raw fallback during conversion.

## Sources and qualification

[ADF structure](https://developer.atlassian.com/cloud/jira/platform/apis/document/structure/) and the [code-block reference](https://developer.atlassian.com/cloud/jira/platform/apis/document/nodes/codeBlock/) describe the target nodes. The official [storage reference](https://confluence.atlassian.com/doc/confluence-storage-format-790796544.html) currently identifies Data Center; it is format evidence, not proof of Cloud editor behavior. Cloud code-macro behavior is also recorded in [Atlassian's Cloud issue](https://jira.atlassian.com/browse/CONFCLOUD-34028). Actual Cloud/editor normalization remains unqualified.

Local tests round-trip supported content and retain visible human edits. Separate acceptance validates representative generated documents against the checksum-pinned full ADF schema from Atlassian's documented schema link. Run `python tools/document-schema-acceptance.py --fetch` once with network access, then omit `--fetch` to reuse the checked fixture. The offline development gate skips this external-fixture test. It does not prove every possible input or live product/editor acceptance. See [import evidence](evidence/document-import.md) and the earlier [renderer evidence](evidence/document-rendering.md).

## Remaining integration

Provider/CLI integration, actual Cloud/editor normalization and domain plan validation remain required under their owning tasks. A render/import result is neither a reviewed plan nor approval. After publication the reviewed visible page remains authoritative; conversion does not authorize overwriting human edits from stale local content.
