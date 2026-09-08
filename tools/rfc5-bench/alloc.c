/* Linux/glibc-only external benchmark observer. Not linked into Outlint.
 * Counts allocation calls and peak live malloc_usable_size bytes, including setup.
 * The benchmark is single-threaded. Failed allocations do not change live bytes.
 */
#include <malloc.h>
#include <stdint.h>
#include <stdio.h>
#include <unistd.h>
extern void *__libc_malloc(size_t);
extern void *__libc_calloc(size_t, size_t);
extern void *__libc_realloc(void *, size_t);
extern void *__libc_memalign(size_t, size_t);
extern void __libc_free(void *);
static size_t live, peak, calls;
static void *record(void *p) {
    if (p) { live += malloc_usable_size(p); if (live > peak) peak = live; calls++; }
    return p;
}
void *malloc(size_t n) { return record(__libc_malloc(n)); }
void *calloc(size_t n, size_t s) { return record(__libc_calloc(n, s)); }
void free(void *p) { if (p) live -= malloc_usable_size(p); __libc_free(p); }
void *realloc(void *p, size_t n) {
    size_t old = p ? malloc_usable_size(p) : 0;
    void *q = __libc_realloc(p, n);
    if (q || n == 0) live -= old;
    return record(q);
}
void *aligned_alloc(size_t a, size_t n) { return record(__libc_memalign(a, n)); }
int posix_memalign(void **p, size_t a, size_t n) {
    if (a < sizeof(void *) || (a & (a - 1))) return 22;
    void *q = record(__libc_memalign(a, n));
    if (!q) return 12;
    *p = q; return 0;
}
__attribute__((destructor)) static void report(void) {
    char out[160];
    size_t saved_peak = peak, saved_calls = calls;
    int n = snprintf(out, sizeof(out), "alloc_calls=%zu peak_usable_bytes=%zu\n", saved_calls, saved_peak);
    if (n > 0 && (size_t)n < sizeof(out)) {
        ssize_t written = write(2, out, (size_t)n);
        (void)written;
    }
}
