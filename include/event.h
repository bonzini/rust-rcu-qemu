#ifndef QEMU_THREAD_H
#define QEMU_THREAD_H

#include <stdbool.h>

typedef struct QemuEvent QemuEvent;

struct QemuEvent {
    unsigned value;
    bool initialized;
};

void qemu_event_init(QemuEvent *ev, bool init);
void qemu_event_set(QemuEvent *ev);
void qemu_event_reset(QemuEvent *ev);
void qemu_event_wait(QemuEvent *ev);
void qemu_event_destroy(QemuEvent *ev);

#endif
