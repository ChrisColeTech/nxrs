// The os layer's view of the generated code's runtime header: the translator's cpu.h (found next on the include
// path), with the SVC, counter and spin-wait entry points routed to the os crate (nxos_svc, nxos_ticks,
// nxos_spin_gate: a spin-wait pass is a scheduler checkpoint). The os layer answers
// what it serves and hands every other SVC to the runtime's rt_svc; the counter runs at the title's rate (19.2 MHz
// for Switch 1 titles, 31.25 MHz for Switch 2), which the os layer sets before any guest code runs.
#pragma once
#include_next "cpu.h"

extern uint32_t nxos_tick_hz;
void nxos_svc(Cpu *c, uint32_t imm, uint64_t pc);
uint64_t nxos_ticks(void);
void nxos_spin_gate(Cpu *c, uint64_t head);

#undef RT_TICK_HZ
#define RT_TICK_HZ nxos_tick_hz
#define rt_svc nxos_svc
#define rt_ticks nxos_ticks
#define rt_spin_gate nxos_spin_gate
