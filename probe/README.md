# Probes run in the original game

`DXProbe.u` is our own UnrealScript probe package (MIT). Its source and the tools that run it in the original game
(Proton on Xvfb, `scripts/probe.sh`) live in the parity archive, `deusex_shin/probe/`.

| File | What |
|---|---|
| `DXProbe.u` | the compiled package, as built by `scripts/probe.sh build` in the archive |
| `expected_crates.txt` | the original's log of `DXProbe.CrateGame` (`scripts/probe.sh crates`): a carried crate dropped against another at four gaps |
| `expected_travel.txt` | the original's log of `DXProbe.TravelGame` (`scripts/probe.sh travel`): Liberty Island -> UNATCO HQ -> Liberty Island, what the player carries and what the levels keep |

`crates/dx-play/tests/travel_probe.rs` and `crates_probe.rs` run the same package in the port and compare the lines.
