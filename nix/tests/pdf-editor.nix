# The PDF editor's document and signatures (servers/files/web/pdf-doc.js,
# pdf-sign.js) under node, on the very MuPDF files the pages load from
# webDist (`packages.web`) and the faces served beside them; poppler's pdfsig checks what is
# signed as a PDF reader would. The PDFs are made inside the test.
{ pkgs, web, ... }:
pkgs.runCommand "pdf-editor"
  {
    nativeBuildInputs = [
      pkgs.nodejs
      pkgs.poppler-utils
    ];
  }
  ''
    cp ${../../servers/files/web/pdf-doc.js} pdf-doc.js
    cp ${../../servers/files/web/pdf-sign.js} pdf-sign.js
    cp ${../../servers/files/web/pdf.test.mjs} pdf.test.mjs
    export HOME=$TMPDIR
    node pdf.test.mjs ${web} ${web}
    touch $out
  ''
