# The PDF editor

Every PDF in Files has **Edit** and **Sign** in its preview and its menu.
They open a full editor over the page: marks, text, the PDF's own words,
forms, signatures, redaction, pages, and reading scans. Like everything
else in a library ([libraries.md](libraries.md)), it all happens in the
tab: the file is opened there, changed there, and sealed there before it
goes back. The box moves ciphertext and never sees the document.

## The pieces

```
servers/files/web/files.js      Edit and Sign on a PDF; loads pdf.js on the click
servers/files/web/pdf.js        the workbench: pages, tools, settings, saving
servers/files/web/pdf-doc.js    the document, on MuPDF: everything that changes it
servers/files/web/pdf-sign.js   signatures: key, certificate, CMS, checking
servers/files/web/pdf-ocr.js    reading a scan's words, on Tesseract
servers/files/web/pdf.css       the workbench's look, from home.css's tokens
servers/files/web/pick.js       a file or folder in Files, from any page with the library; saveInto
servers/files/web/pick.css      the picker's look
```

The editor also opens on a PDF that is not in Files yet (a mail's
attachment): `openEditor({ bytes, name, folder })`. Its Save asks for a
folder (starting at `folder`), and from then on it is a file in Files like
any other. `actions` adds ways out under Save, each given the finished
bytes (Mail's "Sign and reply"). `saveInto(lib, dir, name, bytes)` never
writes over a file: it makes the folders on the way and takes "name (2)"
when the name is taken.

What they load is pinned in `flake.nix` beside our own wasm (`webDist`,
served at `/_dd/web/` and carried by the app):

```
mupdf.js, mupdf-wasm.js, mupdf-wasm.wasm   MuPDF 1.28.1 (AGPL, as Commonty is)   10 MB
Liberation{Serif,Sans,Mono}-*.ttf          from nixpkgs' liberation_ttf          4.5 MB
tesseract.esm.min.js, tesseract-worker.min.js,
tesseract-core-simd-lstm.wasm.js           Tesseract.js 7.0.0, its core 7.0.0    4 MB
eng.traineddata.gz                         the English model (best_int)          3 MB
```

None of it loads with the Files page. MuPDF is fetched as the pointer
reaches Edit, and Tesseract when **Recognise text** is pressed. The gate
serves `.ttf` and `.gz` from `/_dd/web/` for the faces and the model; the
worker comes from our own address (`workerBlobURL: false`), as the content
policy asks.

## What the editor does

- **Marks:** text boxes, highlight, underline, strike-out, squiggle, pen
  and marker, boxes, ovals, lines and arrows, comments, pictures. Each is
  a real PDF annotation, so other readers show it and can change it.
  Select moves and resizes them; Delete removes them.
- **The page's own words** (Edit text): a click opens the paragraph under
  it. On commit `editParagraph` takes the old words out with a redaction
  (text only) and sets the new ones in, so the file holds the new words,
  searchable, and not the old.
  - Paragraphs are found from MuPDF's lines, not its blocks: pieces on
    one baseline are joined, and a line joins the paragraph above it when
    it sits a line below, overlaps it and is the same size. A list item's
    bullet stays where it is.
  - Each word keeps its own style: the new words are matched to the old
    (longest common run); a word that stayed keeps its font, colour and
    size, and a new word takes the style of the one it replaced.
  - The face is ours: Liberation Serif, Sans or Mono, picked from the
    font's name, flags, and for a font that says nothing (TeX's `F37`),
    from its letters' widths (monospaced when `i` is as wide as `m`).
    Each font's words are narrowed or widened (`Tz`, 88-112%) to the
    width its letters took, so lines wrap where they did.
  - Each line keeps its indent and width; justified text stays justified;
    lines before the first change are left alone. When the words run into
    what is below, the editor says so and offers **Shrink to fit** (down
    to 80%) or Undo.
  - Letters a file gives no meaning to (a symbol font's private codes)
    show as boxes, and the editor asks for them to be typed in.
- **Forms:** fields are filled where they are; Tab goes to the next.
  **Add field** makes text, paragraph, check box, date and signature fields.
  A PDF with no fields can be written on with Text and "Part of the page".
- **Sign:** draw, type or photograph a signature (the paper is made
  see-through), place it on the page or in a field's **Sign here**.
  Signatures can be kept for next time. With the key option on (the
  default in a member's own library) the file is also signed so readers
  can check it: see below.
- **Redact:** by area, by words, or by finding: a word, emails, phone,
  card and account numbers, social security numbers. On save MuPDF
  removes what is under each: the text, the parts of pictures it covers
  and line art it covers; marks and comments over it are deleted and
  field values there emptied, the file's Info and XMP metadata (title,
  author, subject, keywords) and the redacted pages' own metadata are
  dropped, and so are the words a tagged PDF keeps for screen readers
  (`/ActualText`, `/Alt`, `/E` in the structure tree). Not touched, and the
  editor says so: bookmark titles and files attached inside the PDF. Save
  replaces the file, and the one before, unredacted, stays sealed in the
  Trash for 30 days. The test checks the bytes, not just the look.
- **Pages:** reorder by dragging (or Alt+arrows), rotate, duplicate,
  delete, a blank page, pages from another PDF in Files or on the device
  (or dropped onto the rail), copy pages out to a new file, split every
  N pages.
- **Find**, with Highlight all and Redact all.
- **Recognise text** on pages that are only a picture: Tesseract reads
  the page drawn at 300 dpi and `addHiddenText` lays the words over it in
  render mode 3, sized to each word's box, so the scan can be searched,
  copied, redacted and edited.
- **Undo and redo** are MuPDF's own journal: every change is one
  operation, pages too.

## Saving

**Save** writes the new file beside the old under a hidden name, moves
the old one to the Trash, then moves the new one into place, so a tab
closed part way loses nothing; Files does not list the hidden name. The
Trash keeps the old one 30 days. **Save as a copy**, **Save flattened**
(marks and fields burnt in) and **Save smaller** (objects compacted, fonts
subset, streams packed) write a new file beside it. **Download** gives the
file to the device, unlocked. A reader of a shared library can mark a
file up and download it; only the owner saves into the library.

A file that already holds signatures is saved as an update after its old
bytes (MuPDF's incremental save), so its signatures still check, and a
reader shows what came after them.

## Signatures

`pdf-sign.js` writes and reads DER itself; WebCrypto does the maths.

- **The key** is a P-256 key made in the tab the first time a member signs
  with it on a device, unexportable (WebCrypto `extractable: false`), with a
  certificate it signs for itself (CN = the name the member types,
  O = Commonty; digital signature and non-repudiation). Both are kept in
  that device's IndexedDB (`commonty-signing`, one entry per account, so
  another member on the same device never signs with it), which holds the
  CryptoKey as it is: the private key never leaves the device, not even sealed. Each
  device has its own key and certificate in the same name; a browser that
  keeps nothing between visits gets one for the tab, and is told so. The
  kept signature pictures are in the library at `Signing/signatures.json`,
  sealed like any file; Files lists only `Files/`, so it does not show.
- **Signing:** `prepareSignature` adds a signature value with a
  `/ByteRange` of `2147483647` marks and a `/Contents` of 16 KiB of zeros,
  into the field signed or a new one over the picture. The file is
  written plainly (`garbage`, or `incremental` if it was signed before and
  nothing is being redacted),
  then `seal` finds the hole by structure: a random tag written into the
  signature's own dictionary must be in the file exactly once, and the
  byte range and zeros are looked for only inside that object. It writes
  the real byte range and puts a CMS
  SignedData over everything else into it: `ETSI.CAdES.detached`, SHA-256,
  ECDSA, with signing time and the ESS signing-certificate-v2 attribute
  (PAdES baseline B).
- **Checking:** when a file opens, each signature is held to its byte
  range (from byte 0, leaving out exactly its own `<hex>` value), the
  certificate its issuer and serial name (no other), the message digest it
  must carry, the content type it signed (data), and the
  signing-certificate-v2 hash when present (signatures from other tools
  often carry none); SHA-1 is refused; then its
  signature is checked against that certificate's key (ECDSA P-256 and
  P-384, RSA PKCS#1 v1.5). The editor says who signed and when and whether
  anything changed after, and whether its certificate was in date when it
  signed. It checks no certificate authority, so a name is always shown as
  the signer's own claim.
- **Redacting a signed file** writes the whole file again: an update after
  the old bytes would keep what was redacted in the earlier revision. Its
  signatures stop checking, and the editor says so before.
- **What a reader shows:** poppler's `pdfsig` says "Signature is Valid"
  and "Total document signed", and that the issuer is unknown. Adobe says
  the same in its words. No authority vouches for the name; the member
  does. A certificate every reader trusts by default would have to come
  from a paid certificate authority, and is not part of this.

## Tests

`checks.pdf-editor` ([`nix/tests/pdf-editor.nix`](../../nix/tests/pdf-editor.nix),
in CI's flake_check job) runs `servers/files/web/pdf.test.mjs` under node
on the very `mupdf.js` and faces that `webDist` serves, with poppler's
`pdfsig` as an outside reader. It makes its own PDFs with MuPDF and covers
marks with undo and redo; a paragraph changed in place (wrapped, its first
line kept, a justified one staying justified, a monospaced word keeping its
face, a list item its bullet); redaction gone from the uncompressed bytes
with the marks, field values, Info and screen-reader text over it; fields
added and filled; every page operation; signing a new field and a field
another signer left (pdfsig: both valid); a changed byte or a wider byte
range failing the check; a redacted signed file written whole; the key
refusing export.

The workbench itself (pdf.js) is driven by hand in headless Chrome before a
change ships, against a library kept in memory and under the gate's content
policy: every tool, save in place and as a copy, signing with `pdfsig` on
the result, forms with Tab, page operations, recognising a scan and finding
in it, dark and phone widths. The gate's page test holds the editor to
scripts from this box alone and to loading on the click.

## Changing it

- A new version of MuPDF or Tesseract is a new pin in `flake.nix` (its
  url and hash); run the editor's tests against it before shipping.
- Anything that changes the file goes in `pdf-doc.js` as one `step`, so
  undo covers it.
- The workbench's pages are carried by the app (`app/carried`); run
  `scripts/sync-app-web` after changing any of them.
