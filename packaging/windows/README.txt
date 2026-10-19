Katna PDF for Windows (@VERSION@)
==================================

A fast PDF reader, editor and signer in the Katna family.
https://github.com/QuakeString/katna-pdf

Run katna-pdf.exe to start it, or drop a PDF on it. To open PDFs with it
by double-clicking, right-click a PDF, choose Open with > Choose another
app, and pick katna-pdf.exe from this folder.

katna-pdf-cli.exe is the command-line tool: run `katna-pdf-cli --help`
in a terminal in this folder.

This build is not signed yet, so Windows may ask before running it the
first time: choose More info, then Run anyway.

To sign with a USB token, install its maker's driver; Katna PDF finds the
usual ones, and the KATNA_PDF_PKCS11 environment variable can name
another.
