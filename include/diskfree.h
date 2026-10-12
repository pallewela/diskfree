// C ABI for the diskfree core (src/ffi.rs). Link against libdiskfree.a.
//
// Handles are opaque and owned by the caller until passed to the matching *_free / *_finish.
// Pointers inside DfNode / DfType stay valid until the next call that mutates the explorer
// (finish, remove, graft, sort). Not thread-safe: use one explorer from one thread.
// Strings are UTF-8; returned names are NOT NUL-terminated (use the *_len fields).

#ifndef DISKFREE_H
#define DISKFREE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

typedef struct DfScan DfScan;         // background scan (Rust ScanJob)
typedef struct DfExplorer DfExplorer; // tree + expanded/filter/rows/insights state

enum { DF_SORT_SIZE = 0, DF_SORT_NAME = 1, DF_SORT_FILES = 2 };
enum { DF_KIND_DIR = 0, DF_KIND_FILE = 1, DF_KIND_SYMLINK = 2, DF_KIND_DENIED = 3 };

typedef struct {
    uint64_t size;        // allocated bytes, rolled up for dirs
    uint64_t files;       // files at any depth
    uint64_t mtime;       // secs since epoch; dirs: newest change inside
    uint64_t parent_size; // for the % bar
    uint32_t parent;      // UINT32_MAX for the root
    uint32_t child_count;
    uint8_t kind;         // DF_KIND_*
    bool expanded;
    bool hit;             // matches the current filter
    const uint8_t *name;
    size_t name_len;
    const uint8_t *hint;  // cleanup hint, NULL if none
    size_t hint_len;
} DfNode;

typedef struct {
    const uint8_t *ext;
    size_t ext_len;
    uint64_t bytes;
    uint64_t files;
} DfType;

// ---- scanning ----
DfScan *df_scan_start(const char *path);
DfScan *df_rescan_start(const DfExplorer *ex, uint32_t id);
void df_scan_progress(const DfScan *job, uint64_t *files, uint64_t *bytes);
void df_scan_cancel(const DfScan *job);
bool df_scan_done(const DfScan *job);
bool df_scan_cancelled(const DfScan *job);
DfExplorer *df_scan_finish(DfScan *job, uint8_t sort);                   // consumes job; blocks
void df_scan_finish_graft(DfScan *job, DfExplorer *ex, uint32_t id);     // consumes job; drops if cancelled
void df_explorer_free(DfExplorer *ex);

// ---- rows & nodes ----
size_t df_row_count(const DfExplorer *ex);
bool df_row(const DfExplorer *ex, size_t i, uint32_t *id, uint16_t *depth);
bool df_node(const DfExplorer *ex, uint32_t id, DfNode *out);
size_t df_path(const DfExplorer *ex, uint32_t id, uint8_t *buf, size_t cap); // returns needed length

// ---- browsing ----
void df_toggle(DfExplorer *ex, uint32_t id, bool open);
void df_set_sort(DfExplorer *ex, uint8_t sort);
void df_set_filter(DfExplorer *ex, const char *query);
int64_t df_reveal(DfExplorer *ex, uint32_t id);                          // row index or -1
void df_remove(DfExplorer *ex, const uint32_t *ids, size_t n);           // after deleting on disk
size_t df_top_level(const DfExplorer *ex, const uint32_t *ids, size_t n, uint32_t *out, size_t cap);
bool df_is_live(const DfExplorer *ex, uint32_t id);

// ---- insights (write up to cap ids, return the full count) ----
size_t df_largest(const DfExplorer *ex, uint32_t *out, size_t cap);
size_t df_candidates(const DfExplorer *ex, uint32_t *out, size_t cap);
size_t df_type_count(const DfExplorer *ex);
bool df_type(const DfExplorer *ex, size_t i, DfType *out);

#endif
