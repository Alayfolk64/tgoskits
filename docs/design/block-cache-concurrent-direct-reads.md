# Concurrent Multi-Folio Block Reads

## Problem

The file page cache performs bounded readahead, but every read spanning more
than one block-cache folio previously acquired the block cache's global
exclusive I/O barrier. The barrier remained held across the synchronous device
request, forced overlapping dirty slots to storage, and serialized otherwise
independent filesystem reads.

The complete Orange Pi self-build profile recorded 99.34 scaled seconds in
file-page fault loading and 51.24 scaled seconds in its underlying block-read
path. The device metrics reported 59,787 submitted requests with a peak of one
in flight. This matches the global exclusion in the multi-folio read path.

## Users and success criteria

StarryOS file faults and buffered reads are the direct users. The change
succeeds when:

- independent multi-folio reads can enter distinct device endpoints at the
  same time;
- a concurrent buffered write remains visible to a direct read even when the
  device returned older bytes;
- reads no longer force dirty cache bytes to storage;
- direct writes and flush retain their existing exclusive durability order;
- one-folio cached reads, cache hits, and error behavior remain unchanged.

## Prior art

Linux commit `980ab36ae5972c83f683b939e50c469c4947229e` does not hold one
device-wide mutex across readahead. `block_read_full_folio()` in `fs/buffer.c`
submits mapped buffers asynchronously, and block-device
`address_space_operations` in `block/fops.c` provide separate `read_folio` and
`readahead` paths. Dirty state is owned by individual buffer heads and folios;
durability is established by writeback and flush, not by forcing writeback
before every read.

## Design

Multi-folio reads now take shared I/O admission. They submit device I/O without
holding a block-cache shard. After completion, they visit only the overlapping
shards and reconcile each cached slot:

- a dirty slot owns newer bytes and copies them into the read result;
- a clean or missing slot accepts the device result and becomes uptodate.

Buffered writes use the same shared admission and shard locks. A write that
finishes before reconciliation is therefore visible in the returned read; a
write after reconciliation is ordered after that read. Flush and direct writes
keep exclusive admission, so they still drain all admitted readers and
buffered writers before establishing device order.

## Alternatives

- Shrinking readahead to one folio avoids the exclusive path but sacrifices
  sequential I/O aggregation and does not address the incorrect lock scope.
- Adding more NVMe queues cannot help while the cache admits only one large
  read globally.
- Eagerly writing dirty slots before reads preserves visibility but adds
  unnecessary writes and turns a read into a durability operation.

## Validation

The deterministic concurrency regression first failed because the second
multi-folio read could not enter device I/O before the first completed.
The repaired path passes the concurrent-read, concurrent-dirty-overlay, and
read-without-forced-writeback regressions. It also passes 339 ax-fs-ng ext4
unit tests, 237 FAT unit tests, and both three-test root-selection matrices.
The three static gates pass, and the 8-core/8-GiB QEMU kernel suite passes all
205 tests.
