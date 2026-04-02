/*
 * Qunix Kernel ABI Test Suite
 * Compiled as a static Linux binary, run under QEMU user-mode to validate
 * that the userland interface behaves as expected.
 *
 * Tests the same syscalls our kernel implements:
 *   read, write, open, close, fork, exec, wait4, pipe, dup2,
 *   mmap, munmap, brk, sigaction, kill, sigreturn, poll
 */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <signal.h>
#include <fcntl.h>
#include <errno.h>
#include <poll.h>

static int passed = 0, failed = 0, total = 0;

#define TEST(name, expr) do { \
    total++; \
    if (expr) { passed++; printf("  PASS  %s\n", name); } \
    else { failed++; printf("  FAIL  %s  (errno=%d: %s)\n", name, errno, strerror(errno)); } \
} while(0)

#define SECTION(name) printf("\n[%s]\n", name)

/* ── Signal handler test ─────────────────────────────────────────── */
static volatile int signal_received = 0;
static void sig_handler(int sig) { signal_received = sig; }

void test_signals(void) {
    SECTION("Signals");

    struct sigaction sa = {0};
    sa.sa_handler = sig_handler;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = SA_RESTART;
    int r = sigaction(SIGUSR1, &sa, NULL);
    TEST("sigaction SIGUSR1", r == 0);

    signal_received = 0;
    kill(getpid(), SIGUSR1);
    TEST("SIGUSR1 delivered", signal_received == SIGUSR1);

    /* SIGINT should also work */
    signal_received = 0;
    sigaction(SIGINT, &sa, NULL);
    kill(getpid(), SIGINT);
    TEST("SIGINT delivered", signal_received == SIGINT);
}

/* ── Pipe test ───────────────────────────────────────────────────── */
void test_pipes(void) {
    SECTION("Pipes");

    int fds[2];
    TEST("pipe()", pipe(fds) == 0);

    const char *msg = "hello pipe\n";
    ssize_t n = write(fds[1], msg, strlen(msg));
    TEST("write to pipe", n == (ssize_t)strlen(msg));

    char buf[32] = {0};
    n = read(fds[0], buf, sizeof(buf)-1);
    TEST("read from pipe", n == (ssize_t)strlen(msg));
    TEST("pipe data correct", strcmp(buf, msg) == 0);

    close(fds[0]);
    close(fds[1]);
}

/* ── Fork + wait test ────────────────────────────────────────────── */
void test_fork_wait(void) {
    SECTION("Fork + Wait");

    pid_t pid = fork();
    TEST("fork()", pid >= 0);

    if (pid == 0) {
        /* child */
        _exit(42);
    }

    int status = 0;
    pid_t wpid = waitpid(pid, &status, 0);
    TEST("waitpid", wpid == pid);
    TEST("exit code 42", WIFEXITED(status) && WEXITSTATUS(status) == 42);
}

/* ── Fork + pipe pipeline test ───────────────────────────────────── */
void test_pipeline(void) {
    SECTION("Pipeline (fork+pipe)");

    int fds[2];
    pipe(fds);

    pid_t pid = fork();
    if (pid == 0) {
        /* child: write to pipe */
        close(fds[0]);
        dup2(fds[1], STDOUT_FILENO);
        close(fds[1]);
        write(STDOUT_FILENO, "data\n", 5);
        _exit(0);
    }

    close(fds[1]);
    char buf[32] = {0};
    ssize_t n = read(fds[0], buf, sizeof(buf)-1);
    close(fds[0]);
    int status;
    waitpid(pid, &status, 0);

    TEST("pipeline read", n == 5);
    TEST("pipeline data", strcmp(buf, "data\n") == 0);
    TEST("pipeline child exit", WIFEXITED(status) && WEXITSTATUS(status) == 0);
}

/* ── mmap / munmap test ──────────────────────────────────────────── */
void test_mmap(void) {
    SECTION("mmap / munmap");

    /* Anonymous private mapping */
    void *p = mmap(NULL, 4096, PROT_READ|PROT_WRITE,
                   MAP_ANON|MAP_PRIVATE, -1, 0);
    TEST("mmap anon", p != MAP_FAILED);

    if (p != MAP_FAILED) {
        memset(p, 0xAB, 4096);
        TEST("mmap write+read", ((unsigned char*)p)[0] == 0xAB);

        int r = munmap(p, 4096);
        TEST("munmap", r == 0);
    }

    /* MAP_FIXED */
    void *fixed_addr = (void*)0x3000000;
    void *q = mmap(fixed_addr, 4096, PROT_READ|PROT_WRITE,
                   MAP_ANON|MAP_PRIVATE|MAP_FIXED, -1, 0);
    TEST("mmap MAP_FIXED", q == fixed_addr);
    if (q != MAP_FAILED) munmap(q, 4096);

    /* Large allocation */
    size_t big = 4 * 1024 * 1024;
    void *large = mmap(NULL, big, PROT_READ|PROT_WRITE,
                       MAP_ANON|MAP_PRIVATE, -1, 0);
    TEST("mmap 4MB", large != MAP_FAILED);
    if (large != MAP_FAILED) {
        ((char*)large)[big-1] = 1;  /* touch last page */
        TEST("mmap 4MB last page", ((char*)large)[big-1] == 1);
        munmap(large, big);
    }
}

/* ── brk test ────────────────────────────────────────────────────── */
void test_brk(void) {
    SECTION("brk / sbrk");

    void *orig = sbrk(0);
    TEST("sbrk(0) valid", orig != (void*)-1);

    void *after = sbrk(4096);
    TEST("sbrk(4096)", after == orig);

    void *new_brk = sbrk(0);
    TEST("brk grew", (char*)new_brk == (char*)orig + 4096);

    /* Write to the new heap area */
    memset(orig, 0xCC, 4096);
    TEST("brk write", ((unsigned char*)orig)[0] == 0xCC);

    /* Shrink back */
    sbrk(-4096);
    void *shrunk = sbrk(0);
    TEST("brk shrink", shrunk == orig);
}

/* ── poll test ───────────────────────────────────────────────────── */
void test_poll(void) {
    SECTION("poll");

    int fds[2];
    pipe(fds);

    /* Non-blocking poll on empty pipe — should not be readable */
    struct pollfd pfd = { .fd = fds[0], .events = POLLIN };
    int r = poll(&pfd, 1, 0);
    TEST("poll empty pipe (no data)", r == 0);

    write(fds[1], "x", 1);

    /* Now should be readable */
    r = poll(&pfd, 1, 0);
    TEST("poll pipe with data", r == 1 && (pfd.revents & POLLIN));

    char c;
    read(fds[0], &c, 1);

    /* Close write end — POLLHUP */
    close(fds[1]);
    r = poll(&pfd, 1, 0);
    TEST("poll POLLHUP after write close", r == 1 && (pfd.revents & (POLLIN|POLLHUP)));

    close(fds[0]);
}

/* ── dup2 test ───────────────────────────────────────────────────── */
void test_dup2(void) {
    SECTION("dup2");

    int fds[2];
    pipe(fds);

    int saved_stdout = dup(STDOUT_FILENO);
    dup2(fds[1], STDOUT_FILENO);
    close(fds[1]);
    write(STDOUT_FILENO, "redirected\n", 11);
    dup2(saved_stdout, STDOUT_FILENO);
    close(saved_stdout);

    char buf[32] = {0};
    ssize_t n = read(fds[0], buf, sizeof(buf)-1);
    close(fds[0]);

    TEST("dup2 stdout redirect", n == 11);
    TEST("dup2 data", strcmp(buf, "redirected\n") == 0);
}

/* ── mremap test ─────────────────────────────────────────────────── */
void test_mremap(void) {
    SECTION("mremap");

#ifdef __linux__
    void *p = mmap(NULL, 4096, PROT_READ|PROT_WRITE,
                   MAP_ANON|MAP_PRIVATE, -1, 0);
    TEST("mremap base mmap", p != MAP_FAILED);
    if (p == MAP_FAILED) return;

    memset(p, 0xBE, 4096);

    /* Grow */
    void *q = mremap(p, 4096, 8192, MREMAP_MAYMOVE);
    TEST("mremap grow", q != MAP_FAILED);
    if (q != MAP_FAILED) {
        TEST("mremap preserves data", ((unsigned char*)q)[0] == 0xBE);
        ((unsigned char*)q)[4096] = 0xEF; /* write to new page */
        TEST("mremap new page writable", ((unsigned char*)q)[4096] == 0xEF);
        munmap(q, 8192);
    }
#else
    printf("  SKIP  mremap (not Linux)\n"); total++;
#endif
}

/* ── COW fork test ───────────────────────────────────────────────── */
void test_cow_fork(void) {
    SECTION("COW fork");

    /* Allocate a page and write to it */
    char *p = mmap(NULL, 4096, PROT_READ|PROT_WRITE,
                   MAP_ANON|MAP_PRIVATE, -1, 0);
    TEST("cow mmap", p != MAP_FAILED);
    if (p == MAP_FAILED) return;
    memset(p, 0xAA, 4096);

    int fds[2];
    pipe(fds);

    pid_t pid = fork();
    if (pid == 0) {
        /* Child: modify our copy, write result to pipe */
        close(fds[0]);
        p[0] = 0xBB; /* trigger COW */
        write(fds[1], &p[0], 1);
        close(fds[1]);
        _exit(0);
    }

    close(fds[1]);
    unsigned char child_val = 0;
    read(fds[0], &child_val, 1);
    close(fds[0]);
    waitpid(pid, NULL, 0);

    TEST("COW child modified", child_val == 0xBB);
    TEST("COW parent unaffected", (unsigned char)p[0] == 0xAA);

    munmap(p, 4096);
}

/* ── Multiple children test ──────────────────────────────────────── */
void test_multi_child(void) {
    SECTION("Multiple children");

    int n_children = 5;
    pid_t pids[5];

    for (int i = 0; i < n_children; i++) {
        pids[i] = fork();
        if (pids[i] == 0) {
            _exit(i + 1);
        }
    }

    int ok = 1;
    for (int i = 0; i < n_children; i++) {
        int status;
        pid_t got = waitpid(pids[i], &status, 0);
        if (got != pids[i] || !WIFEXITED(status) || WEXITSTATUS(status) != i+1)
            ok = 0;
    }
    TEST("5 children all reaped correctly", ok);
}

int main(void) {
    printf("Qunix Kernel ABI Validation Suite\n");
    printf("==================================\n");
    printf("(Running on Linux/QEMU to establish correct expected behavior)\n");

    test_signals();
    test_pipes();
    test_fork_wait();
    test_pipeline();
    test_mmap();
    test_brk();
    test_poll();
    test_dup2();
    test_mremap();
    test_cow_fork();
    test_multi_child();

    printf("\n==================================\n");
    printf("Results: %d/%d passed", passed, total);
    if (failed > 0) printf("  (%d FAILED)", failed);
    printf("\n");

    return failed > 0 ? 1 : 0;
}
