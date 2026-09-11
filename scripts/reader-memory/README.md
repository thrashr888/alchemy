# PDF memory regression

Start Vite, then run `node scripts/reader-memory/check.mjs`. The default URL is
`http://127.0.0.1:8794/scripts/reader-memory/index.html`; pass another URL as
its first argument. The script uses the local Chrome app through the existing
Playwright dependency, without touching the installed Alchemy app or notebooks.

The harness uses the production PDF components and mocks only the backend
commands. It supplies 40 distinct, image-heavy page bitmaps, scrolls through
them, returns to page one, switches documents, resizes, and closes the view.
Checks cover bounded images, concurrency, recovery on revisit, no stale document
images, correct resize requests, and complete observer cleanup.

Memory output includes Chromium JS heap/backing storage and image dimensions.
`decodedPixelBytes` is an estimate from dimensions, not an OS memory reading.
None of these measurements establish the installed WKWebView app's footprint.

A comparison with the previous PdfPageView from commit c713da8 used the same
fixture and scroll sequence: backing storage after 40 pages was 108,434,930
bytes before and 5,888,944 bytes after; page-image requests fell from 874 to 40.
The noisy fixture deliberately compresses poorly so retained bitmap strings are
visible in the measurement. Typical text-only PDFs will save less.
