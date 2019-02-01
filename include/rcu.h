#ifndef QEMU_RCU_H
#define QEMU_RCU_H

#include <stdbool.h>
#include <assert.h>
#include "event.h"
#include "queue.h"
#include "atomic.h"

extern unsigned long rcu_gp_ctr;

extern QemuEvent rcu_gp_event;

struct rcu_reader_data {
    /* Data used by both reader and synchronize_rcu() */
    unsigned long ctr;
    bool waiting;

    /* Data used by reader only */
    unsigned depth;

    /* Data used for registry, protected by rcu_registry_lock */
    QLIST_ENTRY(rcu_reader_data) node;
};

extern __thread struct rcu_reader_data rcu_reader;

static inline void rcu_read_lock(void)
{
    struct rcu_reader_data *p_rcu_reader = &rcu_reader;
    unsigned ctr;

    if (p_rcu_reader->depth++ > 0) {
        return;
    }

    ctr = atomic_read(&rcu_gp_ctr);
    atomic_set(&p_rcu_reader->ctr, ctr);

    /* Write p_rcu_reader->ctr before reading RCU-protected pointers.  */
    smp_mb();
}

static inline void rcu_read_unlock(void)
{
    struct rcu_reader_data *p_rcu_reader = &rcu_reader;

    assert(p_rcu_reader->depth != 0);
    if (--p_rcu_reader->depth > 0) {
        return;
    }

    /* Ensure that the critical section is seen to precede the
     * store to p_rcu_reader->ctr.  Together with the following
     * smp_mb(), this ensures writes to p_rcu_reader->ctr
     * are sequentially consistent.
     */
    atomic_store_release(&p_rcu_reader->ctr, 0);

    /* Write p_rcu_reader->ctr before reading p_rcu_reader->waiting.  */
    smp_mb();
    if (atomic_read(&p_rcu_reader->waiting)) {
        atomic_set(&p_rcu_reader->waiting, false);
        qemu_event_set(&rcu_gp_event);
    }
}

extern void synchronize_rcu(void);

/*
 * Reader thread registration.
 */
extern void rcu_register_thread(void);

/*
 * Support for fork().  fork() support is enabled at startup.
 */
extern void rcu_enable_atfork(void);
extern void rcu_disable_atfork(void);

struct rcu_head;
typedef void RCUCBFunc(struct rcu_head *head);

struct rcu_head {
    struct rcu_head *next;
    RCUCBFunc *func;
};

extern void call_rcu1(struct rcu_head *head, RCUCBFunc *func);

#endif /* QEMU_RCU_H */

