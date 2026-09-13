# Windows mmap and file-locking behavior

The download daemon keeps an open `File` + mutable memory map (`memmap2::MmapMut`)
on each in-progress `{save_path}.part` for the entire lifetime of the job (and
while pause is flushing). This document describes Windows-specific sharing and
locking interactions with the Tauri UI and Explorer.

## What the daemon holds

| Handle | Purpose |
|--------|---------|
| `File` (read/write) | Sparse preallocation (`set_len`), `File::sync_data` after each completed segment |
| `MmapMut` | Lock-free disjoint range writes from parallel segment workers |

The map is only dropped when the job finishes, fails, pauses (`finalize_stop`),
or is cancelled. Until then, Windows associates the mapping with that open
handle.

## Sharing mode

Rust’s standard `OpenOptions` on Windows opens files with a default share mode
that typically allows **other readers** (`FILE_SHARE_READ`) but **not** a second
writer (`FILE_SHARE_WRITE`) and may deny delete (`FILE_SHARE_DELETE`) depending
on CRT/version. Practical consequences:

1. **“Open file location” (Explorer)**  
   Selecting the `.part` folder and showing properties is usually fine.  
   **Double-clicking / previewing** the `.part` itself may open a reader that
   works, or may fail if the previewer requests write/delete access.

2. **Tauri “show in folder” / shell reveal**  
   Safe: it only asks Explorer to select the path. It does **not** open the
   file contents through the UI process.

3. **In-app preview / media probe**  
   If the UI (or a helper) opens `{save_path}.part` with write intent, or opens
   it and then tries to truncate/replace it, Windows can return
   `ERROR_SHARING_VIOLATION` (os error 32). Prefer previewing only **completed**
   files after rename from `.part` → final name, when the daemon has dropped
   the map.

4. **Antivirus / Indexer**  
   Real-time scanners that open files with incompatible share flags can briefly
   contend with `sync_data` or segment flushes. Transient failures are retried
   at the segment layer; persistent AV locks surface as I/O errors.

5. **Deleting a live `.part` from Explorer**  
   Often fails with “file in use” while the daemon holds the map. Cancel the
   job in the app (which drops the map, then deletes) instead of deleting from
   Explorer mid-download.

## Pause / complete / cancel

| Event | Map lifetime | Disk |
|-------|--------------|------|
| Pause | Flushed + dropped in `finalize_stop` | `.part` + atomic `.part.meta` kept |
| Complete | Flushed, verified, dropped | `.part` renamed to final path; meta removed |
| Cancel | Dropped | `.part` / `.part.meta` / legacy `.tmp` removed |

After pause, the UI may open the `.part` for inspection more freely because the
daemon no longer holds the mapping (until Resume).

## Resume-after-crash trust model

Segment completion always:

1. `mmap.flush_range` for the written span  
2. `File::sync_data()` on the `.part` handle  
3. Atomic rewrite of `.part.meta` (temp file in the same directory + `rename`)

A crash between (2) and (3) may leave durable bytes that are not yet listed in
meta — on resume those bytes are re-downloaded for that range (safe, slightly
wasteful). A crash during the meta temp write never corrupts the previous meta
file because `rename` replaces it atomically.

## Guidance for the Tauri UI

- Use shell “reveal in folder” rather than opening `.part` contents.  
- Do not map or memory-map the same `.part` from the UI process while status is
  `downloading` / `probing`.  
- After `completed`, open the final `save_path` only.  
- If a sharing violation appears when the user clicks Preview, show a short
  message: *“File is locked while downloading — pause or wait for completion.”*
