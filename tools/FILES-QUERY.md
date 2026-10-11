# Source13 bounded Files query

Live `filesystem::query_snapshot` holds the actual Store guard through verified
payload reads, sorting, filtering and preview creation. It returns one exact
revision and the total/matched counts with its checked compact file bindings.
The generic `files_query::collect` reads each visible 4KiB payload once and
sorts its metadata and converted preview together; it uses no heap. At most
eight file payloads (64 sectors) are scanned. Failure returns no partial result
and performs no write, format, repair or authority change.

Name and size sorting support ascending/descending order with deterministic
ASCII-insensitive name comparisons and exact-name tie breaks. Search text is
at most31 printable ASCII bytes; matches are ASCII-insensitive substrings.
The default searches names/paths. Explicit content opt-in scans the complete
payload of `/native/` development files, including ASCII byte sequences within
binary files. It does not mistake the128-byte preview for an index. Private
Trash keys are always excluded from results and counts. Preference records
may appear as ordinary verified binary metadata, but their content is excluded
from content search. This is a trusted single-profile bounded store view,
not application path authority or multi-volume search.

The Prism query model separates edited input from the applied query. Only an
exact taken request and confirmed valid result apply a new query. Failure
retains the previous query/results and edited input; stale callbacks, invalid
counts and backwards revisions are rejected. Unrelated file mutations refresh
the applied query. Local queries execute no shell commands and access no
network resources.

`bash tools/test-files-query.sh` runs six actual Store tests: name/size order,
case ties and direction, one-read bounds, zero writes and unchanged full media,
full4KiB binary content matches, private/prefs exclusions, read-only queries,
empty matches, parallel metadata/preview ordering, every40-sector read failure,
unmatched-file corruption and converter failure. Pure model tests cover input
limits, explicit opt-in, failed/stale completion and atomic result publication.

Root main and Prism controls now consume the checked backend and publish
Files-only sorted paths; Home and the trusted portal keep their complete actual
file bindings. The bounded Source13 guest proof below now passes.

`tools/files_query_host.py` prepares the future guest proof without assuming
raw Store slot order: it checks complete retained Prism/next-fs/backend source
manifests, compiles the actual read-only Store/query backend and UI geometry,
then reports the disk revision, exact sorted paths/sizes and dialog controls.
It opens the image read-only, mounts without formatting, denies device writes
and verifies the image hash before/after. A live VM must be paused by its
caller. Four host tests reject changed, missing, extra and escaped source
modules. This host oracle alone is not guest UI evidence. Unicode case folding, per-volume case policy, indexing, timestamps,
grouping, hidden-file controls, multiple volumes and the spec's100ms p95 target
are not implemented or measured by this gate.

`tools/files_query_smoke.py` is prepared against immutable Source13 image
`75b7eee4a19b9451d0c31214ce2a2d0404270ed3e0c02ad03997efc4428709d6`
and manifest `e40e61e7e0be81022ce32b0f314391094466f698080cee3a4183019f9ef42594`.
The actual guest gate passed on its first attempt with 119 checks and 11
retained-source screenshot comparisons (ten applied queries and one Cancel).
It uses actual Find dialog
keyboard/pointer controls and retained-source font pixels, compares every result
with the readonly actual Store backend while the VM is paused, and checks that
queries/Cancel leave durable generation and file bytes unchanged. Its full-content
fixture is saved through actual Editor input and exceeds the 128-byte preview;
a real GUI Trash move and saved binary appearance record test exclusions. A
second actual portal request while Files filtering is active tests that chooser
bindings remain complete. The tool owns a disposable disk and hashes the source
image and all bytes outside the authorized journal prefix. No image was rebuilt. The final report is
`build/thirteenth-query/files-query-smoke.json`; the underlying actual boot
report is `build/thirteenth-query/boot/boot-smoke.json`. Query generation/file
bytes, source image, source manifest, helper source and all bytes outside the
journal prefix remained unchanged. A separate read-only post-run verification
checks the originally recorded helper binaries/libraries and all retained
query/UI source hashes without altering the guest report. The screenshot gate
also confirms exact counts, the separate boot document and zero-result removal
of stale native row text. No physical-device, Unicode or latency claim is added.
