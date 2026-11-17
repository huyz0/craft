# Diagrams

Confluence Cloud has no built-in Mermaid renderer. On a site without a Mermaid app, a `mermaid` macro shows "Error loading the extension!" and a `code` block with language `mermaid` shows the source as plain text. Choose a path in this order:

## 1. A Mermaid app is installed

Some sites have a Marketplace app that renders Mermaid. Find a page that already uses it (`craft confluence page search` for "mermaid", then `craft confluence page get --fields body_storage`) and copy that macro's exact markup: app macros differ in name and parameters. If no page uses one, don't guess a macro name; use path 2.

## 2. Render to an image and attach it (default)

1. Write the diagram in Mermaid and render it locally, for example with Mermaid CLI if it is available and the user allows installing it:

   ```sh
   mmdc -i flow.mmd -o flow.svg -b transparent
   ```

   SVG stays sharp at any zoom. If the SVG renders labels badly, render PNG at 2x instead (`-o flow.png -s 2`). Graphviz, PlantUML or a hand-written SVG work the same way.
2. Upload the file to the page with `confluence.attachment.upload` (`media_type` `image/svg+xml` or `image/png`, `filename` matching what the body references). The page must exist first, so create the page, upload, then update the body; or create the page with the image reference already in place and upload right after.
3. Reference it in the body:

   ```xml
   <ac:image ac:align="center" ac:layout="center" ac:width="600" ac:alt="Release flow: plan, review, create tickets"><ri:attachment ri:filename="flow.svg" /></ac:image>
   ```

4. Keep the source next to it so the next editor can change it:

   ```xml
   <ac:structured-macro ac:name="expand"><ac:parameter ac:name="title">Diagram source (Mermaid)</ac:parameter><ac:rich-text-body><ac:structured-macro ac:name="code"><ac:parameter ac:name="language">text</ac:parameter><ac:plain-text-body><![CDATA[flowchart LR
     A[Plan] --> B[Review] --> C[Create tickets]]]></ac:plain-text-body></ac:structured-macro></ac:rich-text-body></ac:structured-macro>
   ```

To change a diagram later, re-render and use `confluence.attachment.replace` with the same filename; the page body does not change.

Don't send diagram source to an online rendering service (mermaid.ink, kroki.io and similar) unless the user agrees: it publishes the content to a third party.

## 3. No renderer available

Publish the Mermaid source in a `code` block (language `text`) inside an info panel that says it is diagram source, and tell the user that the site needs a Mermaid app or a rendered image. Prefer a short numbered list or table that states the same flow in words, so the page is useful before the diagram exists.

## Diagram hygiene

- Keep one idea per diagram, roughly 5 to 15 nodes. Split larger flows.
- Use left-to-right (`LR`) for processes and top-down (`TD`) for hierarchies.
- Label edges with the condition or data that moves, not "next".
- Give the image an `ac:alt` that states what the diagram shows.
- For whiteboard-style diagrams that people will edit together, a Confluence whiteboard may be better; see the craft-cloud whiteboard reference.
