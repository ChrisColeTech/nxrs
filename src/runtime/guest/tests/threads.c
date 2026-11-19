// Threads, a mutex and a condition variable: four workers bump a counter under the mutex; a producer and a consumer
// pass 100 items through a 4-slot ring; a timed wait on a condition nobody signals times out.
#include "sdk.h"

static Mutex lock;
static int counter;

static void worker(void *arg) {
    int n = (int)(intptr_t)arg;
    for (int i = 0; i < n; i++) {
        mutexLock(&lock);
        int v = counter;
        if ((i & 255) == 0) svcSleepThread(0);   // yield inside the critical section now and then
        counter = v + 1;
        mutexUnlock(&lock);
    }
}

static Mutex ring_lock;
static CondVar not_empty, not_full;
static int ring[4], head, tail, count;

static void producer(void *arg) {
    (void)arg;
    for (int i = 1; i <= 100; i++) {
        mutexLock(&ring_lock);
        while (count == 4) condvarWait(&not_full, &ring_lock);
        ring[tail] = i;
        tail = (tail + 1) % 4;
        count++;
        condvarWakeOne(&not_empty);
        mutexUnlock(&ring_lock);
    }
}

static int consumed, sum;
static void consumer(void *arg) {
    (void)arg;
    while (consumed < 100) {
        mutexLock(&ring_lock);
        while (count == 0) condvarWait(&not_empty, &ring_lock);
        sum += ring[head];
        head = (head + 1) % 4;
        count--;
        consumed++;
        condvarWakeOne(&not_full);
        mutexUnlock(&ring_lock);
    }
}

int main(void) {
    Thread t[4];
    for (int i = 0; i < 4; i++) threadCreate(&t[i], worker, (void *)(intptr_t)10000, 0x10000, 44);
    for (int i = 0; i < 4; i++) threadStart(&t[i]);
    for (int i = 0; i < 4; i++) threadWaitForExit(&t[i]), threadClose(&t[i]);
    printf("counter = %d\n", counter);

    Thread p, c;
    threadCreate(&c, consumer, 0, 0x10000, 44);
    threadCreate(&p, producer, 0, 0x10000, 44);
    threadStart(&c);
    threadStart(&p);
    threadWaitForExit(&p);
    threadWaitForExit(&c);
    printf("consumed %d items, sum %d\n", consumed, sum);

    Mutex m = 0;
    CondVar cv = 0;
    mutexLock(&m);
    uint64_t t0 = armGetSystemTick();
    Result r = condvarWaitTimeout(&cv, &m, 2000000);
    uint64_t ms = (armGetSystemTick() - t0) * 1000 / armGetSystemTickFreq();
    int held = (m & ~0x40000000u) == threadGetCurHandle();
    mutexUnlock(&m);
    printf("timed wait: %s, lock held after: %s, released: %s, waited >= 2 ms: %s\n", r == 0xEA01 ? "timed out" : "?",
           held ? "yes" : "no", m == 0 ? "yes" : "no", ms >= 2 ? "yes" : "no");
    return 0;
}
