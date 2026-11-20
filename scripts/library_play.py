"""library_play.py: the 5B library runs (lane W): long scripted gameplay scenarios on play_scenario.py's harness
(GAME_LOCK, watchdog, control pipe, screenshots, metrics), plus a memory-trend summary.

py scripts/library_play.py <scenario> --exe <game.exe> [play_scenario options...]
py scripts/library_play.py memory <run folder> [--window FROM,TO]   private MB over time from metrics-*.csv

Scenarios here: mk8d_gp (Grand Prix, 50cc, ~11 min of racing), probe (boot + a shot a minute, input-free), menus
(boot, then A every 8 s with shots). Replay files in scripts/inputs.
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import play_scenario as ps  # noqa: E402

MENU_MK8D = [('shot', 9, 'boot'), ('lr', 4, 'title'), ('right', 1.5, 'mii'), ('a', 3, 'first Mii'), ('a', 3, 'menu1'),
             ('a', 3, 'menu2'), ('a', 5, 'menu3'), ('a', 4, 'char'), ('a', 2, 'k1'), ('a', 2, 'k2'), ('a', 4, 'k3'),
             ('a', 4, 'cup'), ('a', 30, 'gp-ok')]
ps.KEYS.update(r='R', l='L', zr='ZR', minus='Minus')
ps.SCENARIOS.update({
    'mk8d_gp': dict(seconds=900, window=(120, 760), steps=MENU_MK8D + [('replay', 30, 'race', 'mk8d_gp.txt')]
                    + [('shot', 60, 'race +%d min' % m) for m in range(1, 12)]),
    'mk8d_cups': dict(seconds=200, window=(60, 150), steps=MENU_MK8D[:-2] + [('shot', 4, 'cups')]
                      + [('right', 2, 'right %d' % i) for i in range(1, 13)] + [('down', 2, 'down')]),
    # the Booster Course: at the cup select (the save remembers the last kart, one step fewer) R turns to its cups
    'mk8d_booster': dict(seconds=330, window=(90, 300), steps=MENU_MK8D[:-3] + [('shot', 3, 'cups'), ('r', 3, 'booster page'),
                         ('a', 3, 'booster cup'), ('a', 30, 'gp-ok'), ('replay', 20, 'race', 'mk8d_gp.txt')]
                         + [('shot', 40, 'race +%d s' % (40 * m)) for m in range(1, 6)]),
    # presses only, no screenshot (a pending screenshot can hang a present: INTERFACES lane G)
    'presses': dict(seconds=330, window=(60, 300), steps=[('a', 8, 'a%d' % i) for i in range(38)]),
    'probe': dict(seconds=420, window=(60, 400), steps=[('shot', 30, 'boot')] + [('shot', 60, 'min %d' % m) for m in range(1, 7)]),
    # Sonic X Shadow Generations: the title, then Shadow Generations (right on the game choice; Sonic Generations is
    # program 1, started by oe::ExecuteProgram), then A through its intro and a run to the right with jumps
    'sonic_shadow': dict(seconds=900, window=(150, 880), steps=[('shot', 40, 'title'), ('a', 8, 'choice'), ('right', 3, 'shadow'),
                         ('a', 20, 'shadow start')] + [('replay', 8, 'skip %d' % i, 'hold_a.txt') for i in range(6)]
                         + [('replay', 30, 'run', 'run_right.txt')] + [('shot', 60, 'play +%d min' % m) for m in range(1, 11)]),
    # Sonic Generations: the title, A on the game choice (program 1, a successor process the harness follows), A
    # through its intro, then a run to the right with jumps
    'sonic_gens': dict(seconds=900, window=(150, 880), steps=[('shot', 40, 'title'), ('a', 8, 'choice'), ('a', 45, 'program 1')]
                       + [('replay', 8, 'skip %d' % i, 'hold_a.txt') for i in range(8)]
                       + [('replay', 30, 'run', 'run_right.txt')] + [('shot', 60, 'play +%d min' % m) for m in range(1, 11)]),
    # Super Mario Party Jamboree: the opening movie (+ to skip), the title (L+R), then A through the first-time
    # menus toward a party or a minigame
    'jamboree': dict(seconds=900, window=(300, 880), steps=[('shot', 40, 'boot'), ('plus', 10, 'skip'), ('plus', 10, 'skip2')]
                     + [x for i in range(45) for x in (('lr', 3, 'lr %d' % i), ('a', 6, 'a %d' % i))]
                     + [('shot', 30, 'end')]),
    # Super Smash Bros. Ultimate: A through the title, Smash, the rules and the stage select (Random) to the character
    # select, then smash_fight.txt: Mario vs a CPU, matches of 2:30 one after another, ~11 min
    'smash_match': dict(seconds=900, window=(150, 840), steps=[('shot', 40, 'boot')] + [('a', 8, 'menu %d' % i) for i in range(7)]
                        + [('shot', 3, 'character select'), ('replay', 30, 'fight', 'smash_fight.txt')]
                        + [('shot', 30, 'fight +%d s' % (30 * m)) for m in range(1, 23)]),
    # Anime Girls: Sun of a Beach: A through the start to the beach, then shooter.txt (walk, turn, fire, jump), ~11 min
    'agsob': dict(seconds=900, window=(120, 840), steps=[('shot', 40, 'boot')] + [('a', 6, 'start %d' % i) for i in range(6)]
                  + [('replay', 30, 'play', 'shooter.txt')] + [('shot', 60, 'play +%d min' % m) for m in range(1, 13)]),
    # Sonic Generations, spin dashing through Green Hill (spindash.txt: a rolling Sonic goes through badniks)
    'sonic_gens2': dict(seconds=900, window=(150, 880), steps=[('shot', 40, 'title'), ('a', 8, 'choice'), ('a', 45, 'program 1')]
                        + [('replay', 8, 'skip %d' % i, 'hold_a.txt') for i in range(8)]
                        + [('replay', 30, 'run', 'spindash.txt')] + [('shot', 30, 'play +%d s' % (30 * m)) for m in range(1, 22)]),
    # Shadow Generations: title, New Game, the story intro skipped (tap A, then hold it: skip_hold.txt), then play (run_jump.txt)
    'shadow_skip': dict(seconds=900, window=(200, 880), steps=[('shot', 40, 'title'), ('a', 8, 'choice'), ('right', 3, 'shadow'),
                        ('a', 25, 'shadow start'), ('down', 3, 'new game row'), ('a', 20, 'new game'), ('a', 20, 'save slot 1')] + [('replay', 18, 'skip %d' % i, 'skip_hold.txt') for i in range(5)]
                        + [('replay', 30, 'run', 'run_jump.txt')] + [('shot', 30, 'play +%d s' % (30 * m)) for m in range(1, 20)]),
    # a visual novel (Beach Bounce Remastered): A every 4 s for ~11 min, a shot each minute
    'vn': dict(seconds=900, window=(60, 720), steps=[('shot', 30, 'boot')] + [x for m in range(11) for x in
               ([('a', 4, 'a %d.%d' % (m, i)) for i in range(14)] + [('shot', 4, 'min %d' % (m + 1))])]),
    # Shadow Generations: title, Shadow, Down + A (New Game), A (save slot 1), the short intro, then boost_run.txt in the
    # first stage (stick forward, ZR boost, B, A), a shot a minute
    'shadow_watch': dict(seconds=1500, window=(300, 1440), steps=[('shot', 40, 'title'), ('a', 8, 'choice'), ('right', 3, 'shadow'),
                         ('a', 25, 'shadow start'), ('down', 3, 'new game row'), ('a', 20, 'new game'),
                         ('a', 20, 'save slot 1'), ('replay', 30, 'play', 'boost_run.txt')]
                         + [('shot', 60, 'play +%d min' % m) for m in range(1, 22)]),
    # Smash: smash_match for 20 min (the memory trend)
    'smash_long': dict(seconds=1500, window=(150, 1440), steps=[('shot', 40, 'boot')] + [('a', 8, 'menu %d' % i) for i in range(7)]
                       + [('shot', 3, 'character select'), ('replay', 30, 'fight', 'smash_fight.txt')]
                       + [('shot', 60, 'fight +%d min' % m) for m in range(1, 22)]),
    # boot only, for a trace (NXOS_TRACE): shots at 60 / 150 / 190 s
    'boot': dict(seconds=200, window=(10, 190), steps=[('shot', 60, 'boot 60 s'), ('shot', 90, 'boot 150 s'), ('shot', 40, 'boot 190 s')]),
    'menus': dict(seconds=420, window=(60, 400), steps=[('shot', 40, 'boot')] + [('a', 8, 'a%d' % i) for i in range(40)]),
})


def memory(run, window=(0, 1e9)):
    rows = []
    for mt in sorted(Path(run).glob('metrics-*.csv')):
        for line in mt.read_text().splitlines():
            f = line.split(',')
            try:
                t, mb = float(f[0]), float(f[4])
            except (ValueError, IndexError):
                continue
            if window[0] <= t <= window[1]:
                rows.append((t, mb))
    if len(rows) < 2:
        return dict(samples=len(rows))
    n = len(rows)
    mt = sum(t for t, _ in rows) / n
    mm = sum(m for _, m in rows) / n
    slope = sum((t - mt) * (m - mm) for t, m in rows) / max(sum((t - mt) ** 2 for t, _ in rows), 1e-9)
    by_min = {}
    for t, m in rows:
        by_min.setdefault(int(t // 60), m)
    return dict(samples=n, first_mb=rows[0][1], last_mb=rows[-1][1], max_mb=max(m for _, m in rows),
                slope_mb_per_min=round(slope * 60, 1), per_minute=by_min)


if __name__ == '__main__':
    if len(sys.argv) > 2 and sys.argv[1] == 'memory':
        w = (0, 1e9)
        if '--window' in sys.argv:
            w = tuple(float(x) for x in sys.argv[sys.argv.index('--window') + 1].split(','))
        print(json.dumps(memory(sys.argv[2], w), indent=1))
    else:
        ps.main()
