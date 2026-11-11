# Hardware

Minimum: 64-bit x86 (amd64), 4 GB RAM, 16 GB storage, Intel or AMD integrated
graphics.

| Tier | Models | Promise |
|---|---|---|
| 1 | ThinkPad X230, T430, X250, T450, T480; Dell Latitude E7450 | Every release is tested on real hardware. |
| 2 | Other laptops with Intel graphics + Intel Wi-Fi | Community reports in `docs/hardware.md`. |
| 3 | NVIDIA (Optimus), Broadcom Wi-Fi, Chromebooks | Not supported. May work. |

Per-model quirks (kernel parameters, firmware, keyboard maps) live in
`image/quirks/<dmi-product>.conf` and are applied at boot by DMI match.

Release checklist on each tier 1 laptop: boot from USB, install, cold boot
time, resume time, lid suspend, Wi-Fi join, sync, power cut test, update +
forced rollback, recovery menu, battery readout, brightness keys, keyboard
layout.
