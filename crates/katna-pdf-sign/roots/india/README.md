# India's root certificates

Built into Katna PDF (`trust.rs`, `Roots::add_india`), so signatures made
with Indian DSC tokens are verified without setup. The roots are the
Controller of Certifying Authorities' (CCA, https://cca.gov.in); the
expired ones stay, since a signature is checked as of when it was made.
`issuers/` holds licensed certifying authorities' issuing certificates
under them, which signers' tokens often leave out of signatures; they are
used to complete chains and are not trusted themselves.

Compare the SHA-256 fingerprints with the CCA's own list when adding or
replacing one:

| File | SHA-256 | Expires |
| --- | --- | --- |
| cca-india-2007.der | F3:75:E2:F7:7A:10:8B:AC:C4:23:48:94:A9:AF:30:8E:DE:CA:1A:CD:8F:BD:E0:E7:AA:A9:63:4E:9D:AF:7E:1C | 2015-07-04 |
| cca-india-2011.der | 2D:66:A7:02:AE:81:BA:03:AF:8C:FF:55:AB:31:8A:FA:91:90:39:D9:F3:1B:4D:64:38:86:80:F8:13:11:B6:5A | 2016-03-11 |
| cca-india-2014.der | 60:10:9B:C6:C3:83:28:59:8A:11:2C:7A:25:E3:8B:0F:23:E5:A7:51:1C:B8:15:FB:64:E0:C4:FF:05:DB:7D:F7 | 2024-03-05 |
| cca-india-2015-spl.der | C3:4C:5D:F5:30:80:07:8F:FE:45:B2:1A:7F:60:04:69:91:72:04:F4:F0:29:3F:1D:72:09:39:3E:52:65:C0:4F | 2025-01-29 |
| cca-india-2022.der | 9A:3F:D3:17:67:98:E8:42:DD:CB:12:C2:62:F1:1C:FA:CC:A7:0A:8B:84:C6:EA:6F:DA:30:84:2A:95:A9:4C:D8 | 2042-02-02 |

Issuers (e-Mudhra): CA 2014 and its Class 3 Organisation 2014 sub-CA;
CA 2022 and its Class 2 / Class 3 Document Signer, Class 3 Individual,
Class 3 Organisation and DGFT 2022 sub-CAs. Other authorities' issuing
certificates (Capricorn, Protean, (n)Code, VSign…) can be added the same
way.
