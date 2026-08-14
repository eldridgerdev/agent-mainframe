# Mouse-wheel scrolling at the plan review gate

Captured from an isolated AMF instance against a throwaway repository using
`scripts/dev/screenshot/scenarios/plan-review-mouse-scroll.txt`. The fixture
seeds a draft that already contains a synthesized plan (`seed_plan_draft.py
--long`), so reaching the review gate costs no agent tokens, and the plan is
deliberately taller than the pane — a plan that fits has its scroll offset
clamped to zero and nothing to show.

The scenario grammar has no mouse step, so the wheel is injected as the raw
SGR bytes a real terminal sends (`ESC [ < 65 ; col ; row M` for a notch down,
button 64 for a notch up), written into the pane with `tmux send-keys -H`.
crossterm parses those into the same mouse event a physical wheel produces.

## 0. Before the fix

The same scenario against a binary built before the change. Four wheel-down
notches have been sent and the pane is still on the plan's first line: the
events fell through to the dashboard list behind the dialog.

![Plan review unmoved after four wheel notches](00-before-four-notches.png)

## 1. The review gate, offset 0

The plan as the gate opens, scrollbar thumb at the top of its track.

![Plan review gate at the top of the plan](01-review-gate-top.png)

## 2. Two notches down

Three lines per notch, matching the debug log, markdown viewer and help
overlay. The title line has scrolled off and `Goal` leads the pane.

![Plan review scrolled six lines](02-wheel-down-two-notches.png)

## 3. Four notches down

`Decisions` and `Architecture` are on screen and the thumb has walked down its
track. Nothing about the dialog's state changed but the offset.

![Plan review scrolled twelve lines](03-wheel-down-four-notches.png)

## 4. One notch back up

Up and down are symmetric: one notch moves three lines the other way, landing
on the `Background` heading.

![Plan review scrolled back up three lines](04-wheel-up-one-notch.png)
