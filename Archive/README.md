# Archive

`MARS.RAR` is the original archive, last modified on 7 February 1998.

The [Historical](../Historical) folder holds exactly the files extracted from this archive, byte for byte, with their original DOS (CRLF) line endings. Git does not keep file timestamps, so the original dates, from 1985 to 1996, survive only in the archive.

Ubuntu's `7z` (7-Zip 23.01) and `bsdtar` can list this archive but can't extract it. RARLAB's `unrar` or WinRAR should work:

```sh
unrar x MARS.RAR
```
