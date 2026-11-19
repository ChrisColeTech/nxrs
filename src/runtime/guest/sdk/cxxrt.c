// A minimal Itanium C++ exception runtime for the tests: a DWARF CFI unwinder over the registered .eh_frame_hdr
// tables (two phases: search, then cleanup with landing pads), __gxx_personality_v0 over the LSDA, and the __cxa_*
// entry points clang's code calls. Single inheritance is matched for class catches; catch (...) catches anything.
#include "sdk.h"

typedef struct {
    uint64_t x[31];
    uint64_t sp, pc;
    uint64_t d[8];   // d8-d15
} Regs;

void __unw_getcontext(Regs *r);
void __unw_resume(const Regs *r) __attribute__((noreturn));

typedef enum {
    _URC_NO_REASON = 0,
    _URC_FATAL_PHASE1_ERROR = 3,
    _URC_END_OF_STACK = 5,
    _URC_HANDLER_FOUND = 6,
    _URC_INSTALL_CONTEXT = 7,
    _URC_CONTINUE_UNWIND = 8,
} _Unwind_Reason_Code;
enum { _UA_SEARCH_PHASE = 1, _UA_CLEANUP_PHASE = 2, _UA_HANDLER_FRAME = 4 };

struct _Unwind_Exception {
    uint64_t exception_class;
    void (*cleanup)(int, struct _Unwind_Exception *);
    uint64_t private1, private2;
} __attribute__((aligned(16)));

typedef struct {
    Regs r;
    uint64_t func_start, lsda, personality, cfa;
} Cursor;
struct _Unwind_Context;

// ---- encoded pointers
enum { DW_EH_PE_omit = 0xff, DW_EH_PE_indirect = 0x80 };

static uint64_t uleb(const uint8_t **p) {
    uint64_t v = 0;
    int s = 0;
    uint8_t b;
    do {
        b = *(*p)++;
        v |= (uint64_t)(b & 0x7f) << s;
        s += 7;
    } while (b & 0x80);
    return v;
}
static int64_t sleb(const uint8_t **p) {
    int64_t v = 0;
    int s = 0;
    uint8_t b;
    do {
        b = *(*p)++;
        v |= (int64_t)(b & 0x7f) << s;
        s += 7;
    } while (b & 0x80);
    if (s < 64 && (b & 0x40)) v |= -((int64_t)1 << s);
    return v;
}

static uint64_t read_enc(const uint8_t **p, uint8_t enc, uint64_t datarel) {
    const uint8_t *at = *p;
    uint64_t v;
    switch (enc & 0x0f) {
    case 0x00: v = *(const uint64_t *)at, *p += 8; break;
    case 0x01: v = uleb(p); break;
    case 0x02: v = *(const uint16_t *)at, *p += 2; break;
    case 0x03: v = *(const uint32_t *)at, *p += 4; break;
    case 0x04: v = *(const uint64_t *)at, *p += 8; break;
    case 0x09: v = (uint64_t)sleb(p); break;
    case 0x0a: v = (uint64_t)(int64_t)*(const int16_t *)at, *p += 2; break;
    case 0x0b: v = (uint64_t)(int64_t)*(const int32_t *)at, *p += 4; break;
    case 0x0c: v = *(const uint64_t *)at, *p += 8; break;
    default: abort();
    }
    if (v) {
        if ((enc & 0x70) == 0x10) v += (uint64_t)at;
        else if ((enc & 0x70) == 0x30) v += datarel;
        if (enc & DW_EH_PE_indirect) v = *(const uint64_t *)v;
    }
    return v;
}

// ---- FDE lookup through the registered .eh_frame_hdr tables
static const uint8_t *eh_hdrs[8];
static int n_eh_hdrs;

void __eh_register(const void *hdr) {
    if (n_eh_hdrs < 8) eh_hdrs[n_eh_hdrs++] = hdr;
}

static const uint8_t *find_fde(uint64_t pc) {
    for (int h = 0; h < n_eh_hdrs; h++) {
        const uint8_t *hdr = eh_hdrs[h], *p = hdr + 4;
        if (hdr[0] != 1 || hdr[3] != 0x3b) continue;   // version 1, datarel sdata4 table
        read_enc(&p, hdr[1], (uint64_t)hdr);
        uint64_t count = read_enc(&p, hdr[2], (uint64_t)hdr);
        const int32_t *t = (const int32_t *)p;
        uint64_t lo = 0, hi = count;
        while (lo < hi) {   // the last entry starting at or below pc
            uint64_t mid = (lo + hi) / 2;
            if ((uint64_t)hdr + (int64_t)t[2 * mid] <= pc) lo = mid + 1;
            else hi = mid;
        }
        if (!lo) continue;
        const uint8_t *fde = hdr + (int64_t)t[2 * (lo - 1) + 1];
        // check the range
        const uint8_t *q = fde + 4;
        const uint8_t *cie = q - *(const uint32_t *)q;
        (void)cie;
        uint64_t start = (uint64_t)hdr + (int64_t)t[2 * (lo - 1)];
        // pc_range: read with the CIE's encoding below; here only the start is known, the caller checks the range
        if (pc >= start) return fde;
    }
    return 0;
}

// ---- CFI
enum { NREG = 96, R_UNDEF = 0, R_SAME, R_OFFSET };
typedef struct {
    uint32_t cfa_reg;
    int64_t cfa_off;
    uint8_t rule[NREG];
    int64_t off[NREG];
} Row;

typedef struct {
    uint64_t code_align;
    int64_t data_align;
    uint32_t ra;
    uint8_t fde_enc, lsda_enc;
    uint64_t personality;
    const uint8_t *insns, *insns_end;
} Cie;

static int parse_cie(const uint8_t *cie, Cie *c) {
    uint32_t len = *(const uint32_t *)cie;
    const uint8_t *end = cie + 4 + len, *p = cie + 8;
    uint8_t version = *p++;
    const char *aug = (const char *)p;
    p += strlen(aug) + 1;
    c->code_align = uleb(&p);
    c->data_align = sleb(&p);
    c->ra = version == 1 ? *p++ : (uint32_t)uleb(&p);
    c->fde_enc = 0;
    c->lsda_enc = DW_EH_PE_omit;
    c->personality = 0;
    if (aug[0] == 'z') {
        uint64_t n = uleb(&p);
        const uint8_t *after = p + n;
        for (const char *a = aug + 1; *a; a++) {
            if (*a == 'R') c->fde_enc = *p++;
            else if (*a == 'L') c->lsda_enc = *p++;
            else if (*a == 'P') {
                uint8_t e = *p++;
                c->personality = read_enc(&p, e, 0);
            }
        }
        p = after;
    }
    c->insns = p;
    c->insns_end = end;
    return 1;
}

static void run_cfi(const uint8_t *p, const uint8_t *end, const Cie *c, uint64_t loc, uint64_t target, Row *row,
                    const Row *initial) {
    Row stack[4];
    int depth = 0;
    while (p < end && loc <= target) {
        uint8_t op = *p++;
        uint8_t hi = op & 0xc0, lo = op & 0x3f;
        if (hi == 0x40) {
            loc += lo * c->code_align;
        } else if (hi == 0x80) {
            row->rule[lo] = R_OFFSET;
            row->off[lo] = (int64_t)uleb(&p) * c->data_align;
        } else if (hi == 0xc0) {
            row->rule[lo] = initial ? initial->rule[lo] : R_SAME;
            row->off[lo] = initial ? initial->off[lo] : 0;
        } else {
            switch (op) {
            case 0x00: break;
            case 0x02: loc += *p++ * c->code_align; break;
            case 0x03: loc += *(const uint16_t *)p * c->code_align, p += 2; break;
            case 0x04: loc += *(const uint32_t *)p * c->code_align, p += 4; break;
            case 0x05: { uint64_t r = uleb(&p); row->rule[r] = R_OFFSET; row->off[r] = (int64_t)uleb(&p) * c->data_align; break; }
            case 0x06: { uint64_t r = uleb(&p); row->rule[r] = R_SAME; break; }
            case 0x07: { uint64_t r = uleb(&p); row->rule[r] = R_UNDEF; break; }
            case 0x08: { uint64_t r = uleb(&p); row->rule[r] = R_SAME; break; }
            case 0x0a: if (depth < 4) stack[depth++] = *row; break;
            case 0x0b: if (depth) { Row r = stack[--depth]; uint32_t cr = row->cfa_reg; int64_t co = row->cfa_off; *row = r; (void)cr; (void)co; } break;
            case 0x0c: row->cfa_reg = (uint32_t)uleb(&p); row->cfa_off = (int64_t)uleb(&p); break;
            case 0x0d: row->cfa_reg = (uint32_t)uleb(&p); break;
            case 0x0e: row->cfa_off = (int64_t)uleb(&p); break;
            case 0x11: { uint64_t r = uleb(&p); row->rule[r] = R_OFFSET; row->off[r] = sleb(&p) * c->data_align; break; }
            case 0x12: row->cfa_reg = (uint32_t)uleb(&p); row->cfa_off = sleb(&p) * c->data_align; break;
            case 0x13: row->cfa_off = sleb(&p) * c->data_align; break;
            case 0x2e: uleb(&p); break;   // GNU_args_size
            default: abort();
            }
        }
    }
}

static uint64_t reg_get(const Regs *r, uint32_t n) {
    if (n < 31) return r->x[n];
    if (n == 31) return r->sp;
    if (n >= 72 && n < 80) return r->d[n - 72];
    abort();
}
static void reg_set(Regs *r, uint32_t n, uint64_t v) {
    if (n < 31) r->x[n] = v;
    else if (n == 31) r->sp = v;
    else if (n >= 72 && n < 80) r->d[n - 72] = v;
}

// Fills func_start/lsda/personality/cfa for the frame at cur->r.pc. 0: no FDE (end of the stack).
static int frame_info(Cursor *cur, Row *row) {
    uint64_t pc = cur->r.pc - 1;
    const uint8_t *fde = find_fde(pc);
    if (!fde) return 0;
    const uint8_t *p = fde + 4;
    const uint8_t *cie = p - *(const uint32_t *)p;
    Cie c;
    parse_cie(cie, &c);
    p += 4;
    uint64_t start = read_enc(&p, c.fde_enc, 0);
    uint64_t range = read_enc(&p, c.fde_enc & 0x0f, 0);
    if (pc >= start + range) return 0;
    uint64_t lsda = 0;
    const uint8_t *end = fde + 4 + *(const uint32_t *)fde;
    if (c.insns_end > c.insns && *(cie + 9) == 'z') {
        uint64_t n = uleb(&p);
        const uint8_t *after = p + n;
        if (c.lsda_enc != DW_EH_PE_omit && n) lsda = read_enc(&p, c.lsda_enc, 0);
        p = after;
    }
    memset(row, 0, sizeof *row);
    run_cfi(c.insns, c.insns_end, &c, start, ~0ull, row, 0);
    Row initial = *row;
    run_cfi(p, end, &c, start, pc, row, &initial);
    cur->func_start = start;
    cur->lsda = lsda;
    cur->personality = c.personality;
    cur->cfa = reg_get(&cur->r, row->cfa_reg) + row->cfa_off;
    return 1;
}

// One frame up: the caller's registers from the saved slots.
static void step(Cursor *cur, const Row *row) {
    Regs n = cur->r;
    for (uint32_t i = 0; i < NREG; i++)
        if (row->rule[i] == R_OFFSET) reg_set(&n, i, *(const uint64_t *)(cur->cfa + row->off[i]));
    n.sp = cur->cfa;
    n.pc = n.x[30];
    cur->r = n;
}

// ---- the _Unwind_* interface the personality uses
uint64_t _Unwind_GetIP(struct _Unwind_Context *ctx) { return ((Cursor *)ctx)->r.pc; }
void _Unwind_SetIP(struct _Unwind_Context *ctx, uint64_t v) { ((Cursor *)ctx)->r.pc = v; }
void _Unwind_SetGR(struct _Unwind_Context *ctx, int n, uint64_t v) { reg_set(&((Cursor *)ctx)->r, (uint32_t)n, v); }
uint64_t _Unwind_GetLanguageSpecificData(struct _Unwind_Context *ctx) { return ((Cursor *)ctx)->lsda; }
uint64_t _Unwind_GetRegionStart(struct _Unwind_Context *ctx) { return ((Cursor *)ctx)->func_start; }

typedef _Unwind_Reason_Code (*Personality)(int, int, uint64_t, struct _Unwind_Exception *, struct _Unwind_Context *);

static _Unwind_Reason_Code phase2(Cursor *cur, struct _Unwind_Exception *ue) {
    for (;;) {
        Row row;
        if (!frame_info(cur, &row)) return _URC_END_OF_STACK;
        if (cur->personality) {
            int actions = _UA_CLEANUP_PHASE | (cur->cfa == ue->private2 ? _UA_HANDLER_FRAME : 0);
            Cursor here = *cur;
            _Unwind_Reason_Code rc = ((Personality)cur->personality)(1, actions, ue->exception_class, ue,
                                                                     (struct _Unwind_Context *)&here);
            if (rc == _URC_INSTALL_CONTEXT) {
                here.r.sp = cur->r.sp;
                __unw_resume(&here.r);
            }
            if (rc != _URC_CONTINUE_UNWIND) return rc;
        }
        step(cur, &row);
    }
}

__attribute__((noinline)) _Unwind_Reason_Code _Unwind_RaiseException(struct _Unwind_Exception *ue) {
    Cursor cur;
    memset(&cur, 0, sizeof cur);
    __unw_getcontext(&cur.r);
    Cursor start = cur;
    // phase 1: find the handler's frame (by its CFA)
    for (;;) {
        Row row;
        if (!frame_info(&cur, &row)) return _URC_END_OF_STACK;
        if (cur.personality) {
            _Unwind_Reason_Code rc = ((Personality)cur.personality)(1, _UA_SEARCH_PHASE, ue->exception_class, ue,
                                                                    (struct _Unwind_Context *)&cur);
            if (rc == _URC_HANDLER_FOUND) {
                ue->private2 = cur.cfa;
                break;
            }
            if (rc != _URC_CONTINUE_UNWIND) return _URC_FATAL_PHASE1_ERROR;
        }
        step(&cur, &row);
    }
    return phase2(&start, ue);
}

__attribute__((noinline)) void _Unwind_Resume(struct _Unwind_Exception *ue) {
    Cursor cur;
    memset(&cur, 0, sizeof cur);
    __unw_getcontext(&cur.r);
    phase2(&cur, ue);
    abort();
}

// ---- the C++ ABI
typedef struct TypeInfo {
    const void *vptr;
    const char *name;
    const struct TypeInfo *base;   // __si_class_type_info only
} TypeInfo;

// type_info vtables: only their addresses matter here (they say which kind of type_info an object is).
const void *_ZTVN10__cxxabiv117__class_type_infoE[4];
const void *_ZTVN10__cxxabiv120__si_class_type_infoE[4];
const void *_ZTVN10__cxxabiv123__fundamental_type_infoE[4];
const TypeInfo _ZTIi = {&_ZTVN10__cxxabiv123__fundamental_type_infoE[2], "i", 0};
const TypeInfo _ZTIPKc = {&_ZTVN10__cxxabiv123__fundamental_type_infoE[2], "PKc", 0};

typedef struct {
    const TypeInfo *type;
    void (*dtor)(void *);
    int64_t selector;
    void *adjusted;   // what __cxa_begin_catch returns: the object, or for a pointer catch the pointer itself
    struct _Unwind_Exception ue;
} Header;   // the thrown object follows ue

#define CLASS 0x434C4E47432B2B00ull   // "CLNGC++\0"

static Header *hdr_of(struct _Unwind_Exception *ue) { return (Header *)((uint8_t *)ue - __builtin_offsetof(Header, ue)); }

void *__cxa_allocate_exception(size_t n) {
    Header *h = malloc(sizeof(Header) + n);
    memset(h, 0, sizeof(Header) + n);
    return h + 1;
}

void __cxa_free_exception(void *obj) { free((Header *)obj - 1); }

void _ZSt9terminatev(void) {
    printf("std::terminate\n");
    abort();
}

void __cxa_throw(void *obj, const TypeInfo *type, void (*dtor)(void *)) {
    Header *h = (Header *)obj - 1;
    h->type = type;
    h->dtor = dtor;
    h->ue.exception_class = CLASS;
    _Unwind_RaiseException(&h->ue);
    printf("uncaught exception\n");
    _ZSt9terminatev();
}

void *__cxa_begin_catch(void *ue_) {
    Header *h = hdr_of(ue_);
    return h->adjusted ? h->adjusted : h + 1;
}

void __cxa_end_catch(void) {}

static int is_si(const TypeInfo *t) { return t->vptr == &_ZTVN10__cxxabiv120__si_class_type_infoE[2]; }

static int matches(const TypeInfo *thrown, const TypeInfo *want) {
    for (const TypeInfo *t = thrown; t; t = is_si(t) ? t->base : 0)
        if (t == want || !strcmp(t->name, want->name)) return 1;
    return 0;
}

_Unwind_Reason_Code __gxx_personality_v0(int version, int actions, uint64_t cls, struct _Unwind_Exception *ue,
                                         struct _Unwind_Context *ctx) {
    (void)version;
    const uint8_t *p = (const uint8_t *)_Unwind_GetLanguageSpecificData(ctx);
    if (!p) return _URC_CONTINUE_UNWIND;
    Header *h = hdr_of(ue);
    if ((actions & _UA_HANDLER_FRAME)) {
        // phase 1 chose this frame: install the handler it found
        _Unwind_SetGR(ctx, 0, (uint64_t)ue);
        _Unwind_SetGR(ctx, 1, (uint64_t)h->selector);
        _Unwind_SetIP(ctx, ue->private1);
        return _URC_INSTALL_CONTEXT;
    }
    uint64_t func = _Unwind_GetRegionStart(ctx), ip = _Unwind_GetIP(ctx) - 1;
    uint8_t lp_enc = *p++;
    uint64_t lpstart = lp_enc == DW_EH_PE_omit ? func : read_enc(&p, lp_enc, 0);
    uint8_t tt_enc = *p++;
    const uint8_t *tt_base = 0;
    if (tt_enc != DW_EH_PE_omit) {
        uint64_t off = uleb(&p);
        tt_base = p + off;
    }
    uint8_t cs_enc = *p++;
    uint64_t cs_len = uleb(&p);
    const uint8_t *cs_end = p + cs_len, *actions_tab = cs_end;
    uint64_t lp = 0, action = 0;
    int found = 0;
    while (p < cs_end) {
        uint64_t start = read_enc(&p, cs_enc, 0), len = read_enc(&p, cs_enc, 0), pad = read_enc(&p, cs_enc, 0);
        uint64_t a = uleb(&p);
        if (ip >= func + start && ip < func + start + len) {
            lp = pad ? lpstart + pad : 0;
            action = a;
            found = 1;
            break;
        }
    }
    if (!found) _ZSt9terminatev();   // a call outside every call site may not throw
    if (!lp) return _URC_CONTINUE_UNWIND;
    int cleanup = 0;
    int64_t selector = 0;
    if (action) {
        const uint8_t *ap = actions_tab + action - 1;
        for (;;) {
            int64_t filter = sleb(&ap);
            const uint8_t *next_at = ap;
            int64_t next = sleb(&ap);
            if (filter > 0) {
                int size = (tt_enc & 7) == 2 ? 2 : (tt_enc & 7) == 3 ? 4 : 8;   // udata/sdata 2, 4, 8 or absptr
                const uint8_t *e = tt_base - filter * size;
                const TypeInfo *want = (const TypeInfo *)read_enc(&e, tt_enc, 0);
                if (!want || (cls == CLASS && matches(h->type, want))) {
                    selector = filter;
                    h->adjusted = want && want->name[0] == 'P' ? *(void **)(h + 1) : 0;
                    break;
                }
            } else if (filter == 0) {
                cleanup = 1;
            }
            if (!next) break;
            ap = next_at + next;
        }
    } else {
        cleanup = 1;
    }
    if (actions & _UA_SEARCH_PHASE) {
        if (!selector) return _URC_CONTINUE_UNWIND;
        h->selector = selector;
        ue->private1 = lp;
        return _URC_HANDLER_FOUND;
    }
    if (!cleanup) return _URC_CONTINUE_UNWIND;
    _Unwind_SetGR(ctx, 0, (uint64_t)ue);
    _Unwind_SetGR(ctx, 1, 0);
    _Unwind_SetIP(ctx, lp);
    return _URC_INSTALL_CONTEXT;
}
