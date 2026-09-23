# SpaceTrace brand

SpaceTrace belongs to the Smet Software Solutions product family. The icon combines exploration (compass and lens) with storage analysis (unequal map compartments). Its warm, illustrated treatment complements Dolly Paste's sheep and MailHarbor's sailing ship.

## Palette

- Smet blue: `#01529A`
- Warm ivory: `#FFFDF8`
- Deep teal: approximately `#125A57`
- Muted mint: approximately `#8EC7B0`
- Warm gold: approximately `#E3AF2A`
- Typography: Segoe UI Variable / Segoe UI / system sans serif.

The parent blue is sourced from the official Smet stylesheet. Other colors are working SpaceTrace tokens and the generated illustration may contain shading.

## Official references

- https://smetsoftwaresolutions.be/
- https://smetsoftwaresolutions.be/logo.png
- https://smetsoftwaresolutions.be/assets/products/dolly-paste.svg
- https://smetsoftwaresolutions.be/assets/products/mailharbor.webp
- https://smetsoftwaresolutions.be/mailharbor/

The existing product marks were design references and are not used as SpaceTrace's logo. Downloaded reference assets are excluded from the public repository; the links above record their provenance.

## Deliverables

- `spacetrace-logo.png`: original generated transparent PNG.
- `../../public/brand/spacetrace-mark.png`: runtime copy.
- `../../src-tauri/icons/`: Windows ICO and required raster sizes generated from the original with Tauri's icon tool.

Generated with the built-in OpenAI image-generation tool. Reference inputs were the official Smet logo and MailHarbor icon; Dolly Paste was described after inspecting its official SVG. No API key or fallback CLI was used.

## Generation prompt

Use case: logo-brand. Create the final standalone app icon for SpaceTrace, a Windows disk-space explorer by Smet Software Solutions. Reference image 1 is the parent Smet brand: strong blue #01529A. Reference image 2 is sibling MailHarbor: elegant approachable illustrated nautical metaphor, dark teal strokes, warm ivory negative spaces. Another sibling Dolly Paste has a charming cream sheep with gold curled horns. Design a distinct new family member, not copies of either existing logo. Main subject: a refined compact magnifying compass / exploration lens, its circular face containing three or four unequal rectangular treemap compartments, conveying discovering where storage space goes. Strong Smet blue outline/silhouette, dark teal and muted mint map compartments, a restrained warm gold compass/star accent. Gently illustrated clean vector-like shapes, confident hand-drawn curves, charming but professional. Simple recognizable silhouette that works at small Windows taskbar sizes, consistent stroke weights, generous negative space. Square centered icon composition with the icon filling about 80% of the canvas. Genuinely transparent background outside the icon, warm ivory within the lens is allowed. No background tile, no wordmark, no letters, no text, no photorealism, no 3D, no mockup, no drop shadow, no watermark. Output one high-quality icon only.
