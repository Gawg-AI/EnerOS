/* 49 spike：freestanding 冒烟 shim——仅证 ISA 兼容，非移植方案 */
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>
#define CLOCK_MONOTONIC 1
struct timespec { long tv_sec; long tv_nsec; };
static inline int clock_gettime(int clk, struct timespec *ts) {
    (void)clk;
    static long t = 0; t += 1000;
    ts->tv_sec = t / 1000000000L; ts->tv_nsec = 0;
    return 0;
}
/* pthread/sched 桩（单线程语义替换点清点——spike 证据） */
#define GGML_USE_THREADS_SHIM 1
typedef struct { int _x; } shim_mutex_t;
typedef struct { int _x; } shim_cond_t;
typedef struct { int _x; } shim_thread_t;
#define pthread_mutex_t shim_mutex_t
#define pthread_cond_t shim_cond_t
#define pthread_t shim_thread_t
#define pthread_attr_t shim_thread_t
static inline int pthread_mutex_init(shim_mutex_t *m, void *a) { (void)m; (void)a; return 0; }
static inline int pthread_mutex_lock(shim_mutex_t *m) { (void)m; return 0; }
static inline int pthread_mutex_unlock(shim_mutex_t *m) { (void)m; return 0; }
static inline int pthread_mutex_destroy(shim_mutex_t *m) { (void)m; return 0; }
static inline int pthread_cond_init(shim_cond_t *c, void *a) { (void)c; (void)a; return 0; }
static inline int pthread_cond_wait(shim_cond_t *c, shim_mutex_t *m) { (void)c; (void)m; return 0; }
static inline int pthread_cond_broadcast(shim_cond_t *c) { (void)c; return 0; }
static inline int pthread_cond_destroy(shim_cond_t *c) { (void)c; return 0; }
static inline int pthread_create(shim_thread_t *t, void *a, void *(*f)(void*), void *arg) { (void)t;(void)a; f(arg); return 0; }
static inline int pthread_join(shim_thread_t t, void **r) { (void)t; (void)r; return 0; }
static inline void pthread_exit(void *r) { (void)r; }
static inline int sched_yield(void) { return 0; }
