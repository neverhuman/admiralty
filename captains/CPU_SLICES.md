# xbabe2 CPU slices for the max-parallel-code phase (set by Grok Bot for Jepson, Oct 17 2026, 9:28 PM MDT)

xbabe2 has 128 logical CPUs (siblings are i and i+64) and 125 GiB of RAM. The allowed agent range is CPUs 0-50 and 64-114, which is 102 logical CPUs. At 03:30Z these were already pinned or reserved, so leave them alone: 0-5, 16-21, 32-43 (Joil workers plus the Joil lane pool 36-39), 68-77, 90-91 and 110-111 (Joil lane pool). CPUs 51-63 and 115-127 are outside the agent range. Don't use them.

Each lane uses 2 CPUs. That leaves about 64 free logical CPUs, enough for about 32 new lanes, gated by load and RAM.

| Project | Slice (logical CPUs) | Lane pairs |
|---|---|---|
| JopeDime | 22-31, 92-101 | 10 |
| DOPE | 78-89 | 6 |
| Jarambit | 6-15 | 5 |
| Joil | lease pool only: 36-37, 38-39, 90-91, 110-111 (renewal-002; the lease caps Joil at 12 cores / 36 GiB / 15 GiB scratch) | 4 |
| SHARED overflow | 44-50, 64-67, 102-109, 112-114 | about 10 |

Joil may NOT use overflow CPUs. Its lease cap is binding until Jepson raises it.

Launch guard, checked before every launch: the 1-minute load must be at or below 85 and MemAvailable at or above 25 GB. For Joil, its lease's stricter guard (load at or below 70, MemAvailable at or above 40 GiB) still applies. Stagger launches about 60 seconds apart.

## Overflow claims (append one line per 2-CPU pair BEFORE using it: `<cpus> <project> <screen> <UTC>`)
44,45 DOPE dope_lane_w02 2026-10-18T03:43:46Z
102,103 JopeDime jope_i06_codex 2026-10-18T03:44:30Z
104,105 JopeDime jope_i10_codex 2026-10-18T03:44:30Z
106,107 JopeDime jope_i12_codex 2026-10-18T03:44:30Z
108,109 JopeDime jope_i13_codex 2026-10-18T03:44:30Z
46,47 DOPE dope_lane_c04 2026-10-18T03:44:50Z
48,49 DOPE dope_lane_f01 2026-10-18T03:45:54Z
64,65 DOPE dope_lane_m00 2026-10-18T03:46:58Z
112,113 JopeDime jope_i07_codex 2026-10-18T04:01:30Z
66,67 DOPE dope_lane_u03 2026-10-18T04:04:23Z
112,113 JopeDime jope_i07_codex RELEASED 2026-10-18T04:07:45Z; CPU 112 occupied by another workload, no test launched
112,113 JopeDime jope_i07_codex 2026-10-18T04:14:10Z
112,113 JopeDime jope_i07_codex RELEASED 2026-10-18T04:16:00Z; captain directive assigns CPUs 18,19; 112,113 not used
102,103 JopeDime jope_i06_codex RELEASED 2026-10-18T09:50:30Z; screen finished/closed
104,105 JopeDime jope_i10_codex RELEASED 2026-10-18T09:50:30Z; screen finished/closed
106,107 JopeDime jope_i12_codex RELEASED 2026-10-18T09:50:30Z; screen finished/closed
108,109 JopeDime jope_i13_codex RELEASED 2026-10-18T09:50:30Z; screen finished/closed
102,103 admiralty captain_admiralty 2026-10-18T09:50:30Z
104,105 admiralty adm_lane_1_ui 2026-10-18T09:50:30Z (reserved standby, awaits frozen capsule)
106,107 admiralty adm_lane_2_testkit 2026-10-18T09:50:30Z (reserved standby, awaits frozen capsule)
108,109 admiralty adm_lane_3_session 2026-10-18T09:50:30Z (reserved standby, awaits frozen capsule)
64,65 DOPE dope_lane_m00 RELEASED 2026-10-18T10:42:00Z; screen finished/closed
66,67 DOPE dope_lane_u03 RELEASED 2026-10-18T10:42:00Z; screen finished/closed
104,105 admiralty adm_p13a_workplans 2026-10-18T10:34:40Z (active Screen lane ADM-P13A)
106,107 admiralty adm_p05b_questions 2026-10-18T10:35:58Z (active Screen lane ADM-P05B)
108,109 admiralty adm_p09a_workspace 2026-10-18T10:37:16Z (active Screen lane ADM-P09A)
112,113 admiralty adm_extra_1 2026-10-18T10:42:00Z (pre-authorized standby for P04A/P10A concurrent Screen if needed)
64,65 admiralty adm_extra_2 2026-10-18T10:42:00Z (pre-authorized standby for P04A/P10A concurrent Screen if needed)

112,113 admiralty Root-source-qualification 2026-10-20T03:22:31.282505+00:00 (finite d1659f4 demo/Store tests; reuse project standby adm_extra_1; max2 compiler jobs, no other Root Linux build, release back to standby on completion)

112,113 admiralty Root-source-qualification RELEASED BACK TO adm_extra_1 PROJECT STANDBY 2026-10-20T03:40:23.898190+00:00; own finite coordinator inactive, exact d165 demo2/2 and Store238/238 PASS, no other project reservation released.

112,113 admiralty Root-browser-reproduction 2026-10-20T03:58:51.608125+00:00 (reuse prior project standby; own finite private Node24.22+native build+scoped7 E2E controls; max2 compiler jobs/testthreads1; no other Root Linux job; release back on completion)

112,113 admiralty Root-browser-reproduction RELEASED BACK TO adm_extra_1 PROJECT STANDBY 2026-10-20T04:05:05.584958+00:00; own Node provisioning/native build/scoped7/full30 commands complete, coordinators586475/749917 inactive. No other reservation released.

112,113 admiralty Root-Chromium145-reproduction 2026-10-20T04:59:42.998963+00:00 (reuse adm_extra_1 standby; finite private source75 copy; exact Node24.22/Chromium145; max2 compiler jobs, no other Root Linux job; no service/runner control)

112,113 admiralty Root-Chromium145-reproduction RELEASED BACK TO adm_extra_1 PROJECT STANDBY 2026-10-20T05:07:55.440658+00:00; finite source75/Chromium145 full32PASS and candidate Chrome155 full32PASS; own coordinators3161138/3534248/3792368 inactive, no Root Linux job remains; no other reservation released.

112,113 admiralty Root-startup-sync-qualification 2026-10-20T05:24:34.076374+00:00 (reuse own project standby, finite Store probe/build/strace; max2 compiler jobs/testthreads1, no service/runner control, release on completion)

112,113 admiralty Root-startup-sync-qualification RELEASED BACK TO adm_extra_1 PROJECT STANDBY 2026-10-20T05:39:45.350816Z; finite coordinators1633102/2101259/2706784 verified inactive. Baseline19sync/~3sec; final70 9sync/~0.11sec and Linux32PASS. No other reservation released.

112,113 admiralty Root-capture-overflow-reproduction 2026-10-20T06:43:18.470257+00:00 (reuse adm_extra_1 standby; finite exact6c9 runtime-source Linux unit reproduction, two compiler jobs/tests1, no other Root Linux build, no service/runner controls; return standby on completion)

112,113 admiralty Root-capture-overflow-reproduction RELEASED BACK TO adm_extra_1 PROJECT STANDBY 2026-10-20T07:07:08.845955+00:00; own finite coordinators2148336/2489502/3725721/3977852 verified inactive; candidate Linux115Serverpass/2ignored, Runtimecap1pass/strictClippy pass; no other reservation released.

6,7,10,11 jarambit existingRoot JARAMBIT-LOCAL-73FFEF00-XBABE2-20261020T185733Z 2026-10-20T18:57:33.057661+00:00 manualSERIALsoftwarevalidation exact73ff/tree58fdee39; versionCPU6/2GiB,d1<=4CPU/8GiB,swap0,nice19,noGPU,newallocated8GiB/newapparent12GiB,no reset. Mustconsume before19:15Z orunusedno-launch; expires20:30Z/max90min. Metadataresourcegrant only, actualOS/governance/preflight mandatory; no sharedrunner/service/sibling changes. CPU8/9/projectworkers andallforeignleases preserved. Receipt /home/ubuntu/xbabe2_kicker/captains-20261017/resource-allocations/JARAMBIT-LOCAL-73FFEF00-XBABE2-20261020T185733Z.json SHA256:dbd421e0c335fa27dde0307c0dca4cd8914947a618cf9d36a0834ce6f7e0e47d.

## 2026-10-20T19:16:18.204268+00:00 | JARAMBIT-LOCAL-73FFEF00-XBABE2-20261020T185733Z first-consumption window CLOSED
No valid owner attempt/admission seen by19:15:25Z: required attempt directory absent and no current lane process. No late launch under original grant; no reset/extension, original20:30hardexpiry preserved. Original predeadline admission claims require actual timestamped owner/PID/cgroup/argv evidence. Original grant bytes unchanged. Root route pending; no other reservation/process touched. Receipt /home/ubuntu/xbabe2_kicker/captains-20261017/JARAMBIT_PAPER_V4_REVIEW_20261020/admiral-smoothing-owner-and-four-host-readback-20261020T1909Z/ACTUAL_1915_FIRST_CONSUMPTION_WINDOW_CLOSED_READBACK.json

## 2026-10-20T19:46:01.160143+00:00 | JARAMBIT-DBC4034-OWNJOB-NEUTRAL-002
LogicalCPU6 only (no sibling/physicalexclusiveclaim), ownfinite90s root-scope qualification underactual70fleetcap; no artifact/science admission. Source dbc4034685a86ed3a849ca5b526fc3af5ffb8a0a; firststart 2026-10-20T19:49:01.160143+00:00; expiry 2026-10-20T19:51:01.160143+00:00. Exactownunit-onlylimits/cleanup afterprovenance; no foreignaction. Authority /home/ubuntu/xbabe2_kicker/captains-20261017/JARAMBIT_PAPER_V4_REVIEW_20261020/admiral-governed-artifact-route-dbc4034-20261020T1930Z/ACTUAL_NEUTRAL_OWN_SCOPE_PROBE_AUTHORITY_002.json

## 2026-10-20T19:57:22.210668+00:00 | JARAMBIT own-job neutral current005
LogicalCPU6 only within existing project6-15, quota100%,2GiB/swap0/nice19/pools1. No physicalexclusive/sibling claim. Actual002-004 terminal no payload; current005 fresh finite90s creation-time owned root scope only, firststart20:01:47Z/fixedexpiry20:03:47Z. Authority /home/ubuntu/xbabe2_kicker/captains-20261017/JARAMBIT_PAPER_V4_REVIEW_20261020/admiral-governed-artifact-route-dbc4034-20261020T1930Z/ACTUAL_PRECONFIGURED_NEUTRAL005_AUTHORITY.json. Old73 allocation UNUSED and not renewed; all foreign reservations unchanged.

## 2026-10-20T20:03:40.081462+00:00 | JARAMBIT-OBSERVATION-PREFLIGHT-6C49 effective
Priorneutral002-006 terminal, exactownscopesinactive/no payloadongoing. Captain soleoperator LogicalCPU6/quota100%/2GiB/swap0/nice19/pools1, ONE actualobservation preflight6c49 only; firststart20:11:50.958436Z/fixedexpiry20:21:50.958436Z/max600s. No sibling/exclusiveclaim/foreignchanges. Aggregate8GiBallocated/12GiBapparent counters/baselineunchanged, nofailurereset. Grant /home/ubuntu/xbabe2_kicker/captains-20261017/JARAMBIT_PAPER_V4_REVIEW_20261020/admiral-governed-artifact-route-dbc4034-20261020T1930Z/JARAMBIT_OBSERVATION_PREFLIGHT_6C49_ACTUAL_GRANT.json. Actualconsumption stillunverified; do not launch duplicates.

## 2026-10-20T20:12:01.183899+00:00 | JARAMBIT6c49 observation grant ACTUALLY CONSUMED/TERMINAL
Amended299c5120 body consumed within originalwindow; actual20:10:17.308856Z exit0, originalinsideACK/caps verified, ownscopeinactive/reaped/cleanup0. LogicalCPU6 ownjob returned to existingJarambitprojectpool; no sibling/foreignlease release. Singleattemptcannotrerun; old20:21:50expiry and originalbudgethistory retained. Currentusage {"utc": "2026-10-20T20:10:17.108220+00:00", "usage": {"allocated": 521900032, "apparent": 518647251}, "delta": {"allocated": 1208320, "apparent": 1012081}} . Actualreceipt /home/ubuntu/xbabe2_kicker/captains-20261017/JARAMBIT_PAPER_V4_REVIEW_20261020/admiral-governed-artifact-route-dbc4034-20261020T1930Z/ADMIRAL_ACTUAL_6C49_PREFLIGHT_CONSUMED_EXIT_ZERO.json . Nextphase independentRootQ read-only; futuregeneration needsnewexactsource/bodygrant with SAMEaggregateoperationcounters, noreset.

## 2026-10-20T20:17:43.173926+00:00 | Finite affected D1neutral007 only
Logical10,11,12,13 withinJarambit6-15, max4quota/8GiB/swap0/nice19/nativepools1; NO physicalexclusive/siblinglease.90sneutralidentityprint only, source/cache writesnone. Firststart2026-10-20T20:20:43.173926+00:00 expiry2026-10-20T20:22:43.173926+00:00. Sameaggregateoperationbaseline noreset. Actualsoftware finalreservationNOTissued untilcompletefreeze/body/privateRootlane. Authority /home/ubuntu/xbabe2_kicker/captains-20261017/JARAMBIT_PAPER_V4_REVIEW_20261020/admiral-governed-artifact-route-dbc4034-20261020T1930Z/ACTUAL_AFFECTED_D1_NEUTRAL007_AUTHORITY.json

## 2026-10-20T20:35:35.579788+00:00
JARAMBIT generation26a: LogicalCPU6 only/no sibling-exclusiveclaim,1CPUquota/2GiB/swap0/nice19/pools1. ONE exact a21e740d makeall; firststart20:41:12Z/fixedexpiry21:01:12Z/max1200s. Old preflight/Root metadata spent terminal; original growth costs retained. Grant /home/ubuntu/xbabe2_kicker/captains-20261017/JARAMBIT_PAPER_V4_REVIEW_20261020/admiral-governed-artifact-route-dbc4034-20261020T1930Z/ADMIRAL_SHARED_GENERATION_26AEDA0_SINGLE_JOB_GRANT.json

## 2026-10-20T20:37:50.438033+00:00
D1neutral007 TERMINAL20:17:43Z exit0/inactive/cleanup0: logical10-13 returned to Jarambitprojectpool only; no foreignlease release. Metadata Rootbfd TERMINAL20:29:23Z exit0/inactive, CPU6 returned then ONE26ageneration941a8d44 CPU6/1CPU2GiB/swap0/nice19/pools1 effective20:35, firststart20:41:12Z/expiry21:01:12Z/max1200. No physicalexclusive/siblinglease. Allspentgrowth preserved.

## 2026-10-20T20:46:17.162148+00:00
First26ageneration terminalFAILED20:38:50Z andinactive; CPU6 returned toprojectpool then freshONE correctedretry831466 CPU6/1CPU2GiB/swap0/nice19, firststart20:50:18Z/fixedexpiry21:01:12Z. Exactlymanifest outputrolecorrected; allspentgrowth/failedarchivescharged, no reset/siblingrelease.

## 2026-10-20T20:49:32.097314+00:00 | JOIL E26 fresh resource allocation
ACTUAL E26 resource decision: xbabe2 logical CPUs 36,37 (two distinct physical cores; SMT100/101 NOT granted), CPU quota200%,12GiB/no-swap,nice10,Cargo2/tests2,noGPU,5GiB TOTAL new apparent+allocated aggregate, fixed expiry 2026-10-20T22:19:32.097314+00:00, first start 2026-10-20T20:59:32.097314+00:00. Source23e167/tree40fa; exact proposed Root-only worktree /home/ubuntu/joil/.agent/worktrees/e26-linux-validation-20261020 is ABSENT, not installed/claimed yet. Read /home/ubuntu/xbabe2_kicker/captains-20261017/fleet-accountability-20261020T2043Z/joil/JOIL_E26_XBABE2_FINITE_ALLOCATION_ACTUAL_DECISION.json SHA256 cf9279c230680370203b4e10ac7ee19a729c91f7f442c47d568c0d1a6f9c8cb6. Root must bind/custody source path/body/actual own-job kernel limits/PID+unit/aggregate guard before payload; global run-capped alone is insufficient. This is an effective finite resource reservation, NOT actual launch or native dispatch. Root should return exact admission proof by 2026-10-20T20:57:32.097314+00:00. Preserve existing Captain/canonical/private fixture clone, other jobs, all failed bytes and old unused expired leases; no source/CI job launched by Admiral.

## 2026-10-20T20:54:08.034488+00:00
Corrected26a generation ACTUALLY TERMINAL20:52:54Z exit0/inactive/cleanup0. LogicalCPU6 returned to existingJarambitprojectpool only; no sibling/foreignreservation released. Singleattempt spent, original21:01expiry retained and no rerun. Alloutputcosts/firstfailure preserved; Root future finalvalidation needs its own exactfreeze/body/privatecache grant.

## 2026-10-20T21:13:14.545459+00:00 | JOIL E26 explicit late-start amendment
EXPLICIT LATE-START AMENDMENT EFFECTIVE 2026-10-20T21:13:14.545459+00:00: consumed Root21:10 request, originalmissed20:59 firstwindow remainsrecorded; ZERO payload/staging verified and proposedpathabsent. SAME source23e167/tree40fa,CPU36/37/quota200%/12GiB/swap0/nice10/Cargo2/tests2/noGPU/5GiBaggregate/4.5GiBearlystop. First-start now 2026-10-20T21:33:14.545459+00:00; fixedexpiry22:19:32 UNCHANGED/max90min clippedtoexpiry. No extra ACK for ordinary authorizedpreparation; Rootbindactualbody/tool/source/PID/unit/kernel/outsideoutputguardbeforepayload. Currentdecision /home/ubuntu/xbabe2_kicker/captains-20261017/fleet-accountability-20261020T2043Z/joil/JOIL_E26_XBABE2_FINITE_ALLOCATION_ACTUAL_DECISION.json SHA b9db36a72ef0cd0886daf38afcab30a98b864496a7ce9877716b4c8b35ff5a92. Old cf9279 bytes archived, no reset or retroactiveleaseadmission. RootactualCOORD route works; no new localRootqueue attempt.


## 2026-10-20T21:18:25.305074+00:00 | Jarambit4213 hash-only closure terminal
Root-owned closure actual exit0 at21:06:33, inactive/cleanup0; CPU6 returned to existing Jarambit project pool only. Single-job closure grant spent, no rerun or release of foreign/sibling reservations. Root software jobs still need exact current body/path/kernel admission.


## 2026-10-20T21:34:54.879993+00:00 | CURRENT EFFECTIVE E26 START WINDOW — NOT PENDING HUMAN APPROVAL
Root21:29statement that21:10request is pending is superseded by actual21:13b9db36 grant and this explicit cadence-alignment amendment. Current exactdecision /home/ubuntu/xbabe2_kicker/captains-20261017/fleet-accountability-20261020T2043Z/joil/JOIL_E26_XBABE2_FINITE_ALLOCATION_ACTUAL_DECISION.json SHA256 98719dca246bfd92d476fb6f3331e2c62acc0054d1ed6e62446a39e444234246. SAME source23e167/tree40fa, xbabe2logical36/37/quota200%,12GiB/swap0,nice10,Cargo2/tests2,noGPU,5GiBaggregate/4.5GiBearlystop; proposedRoot-ownedworktree/cache/logpaths unchanged. First-start deadline now2026-10-20T22:09:32.097314+00:00, fixedexpiry22:19:32UNCHANGED, allactualruntime clippedtoexpiry. Both earliermissedstartwindows and oldgrantbytes preserved, ZERO Linuxpayload/source-staging observed, noreset/increasedcap/renewedexpiry. Fresh hostCPU7.3%,Mem92.0GiB,fleetcapunfrozen,no narrowCPU36/37conflict; no otherprojectaction.
Root may now consume latestexactdecision and perform ordinaryauthorizedownedsource/body/kernel/outputguard/PIDadmission, no extrahumanACK. Returnactualbody/admissionorprecise technicalgap at21:37Z; no blindrerun. New21:29detailedplan annex/mesher/physics proposals are separatelyunadmitted and NOTcovered by this existingsoftwarevalidationgrant. ExistingCLOSURE003/PR50remainsclosed.


## 2026-10-20T21:51:12.583276+00:00 | ONE EFFECTIVE 4ff observation allocation — no duplicateRootgrant/job
ACTUAL fresh4ff observation allocation EFFECTIVE. ExistingCaptain sole canonicalexecutor: /home/ubuntu/xbabe2_kicker/captains-20261017/JARAMBIT_PAPER_V4_REVIEW_20261020/admiral-governed-artifact-route-dbc4034-20261020T1930Z/ADMIRAL_EXACT_4FF1CE1_OBSERVATION_SINGLE_JOB_GRANT.json SHA4b25c3d1c53811c3af89c221afabc1e95772f3865ee7423b0640980244c18742; config /home/ubuntu/xbabe2_kicker/captains-20261017/JARAMBIT_PAPER_V4_REVIEW_20261020/admiral-governed-artifact-route-dbc4034-20261020T1930Z/ADMIRAL_EXACT_4FF1CE1_OBSERVATION_SINGLE_JOB_CONFIG.json SHA42edf09cf1528e9016248ad1de1f0d9858173743a4fc7b924c0014400903725e. Root2132sourceauthority/currentclean4ff/tree1511, exact28bd0bc2 body, proven47409route. Execute ONLY /bin/bash /home/ubuntu/xbabe2_kicker/captains-20261017/JARAMBIT_PAPER_V4_REVIEW_20261020/admiral-governed-artifact-route-dbc4034-20261020T1930Z/preconfigured_scope.sh /home/ubuntu/xbabe2_kicker/captains-20261017/JARAMBIT_PAPER_V4_REVIEW_20261020/admiral-governed-artifact-route-dbc4034-20261020T1930Z/ADMIRAL_EXACT_4FF1CE1_OBSERVATION_SINGLE_JOB_CONFIG.json after exactimmutable/source/control checks. CPU6 shared/nonexclusive1CPU/2GiB/swap0/nice19/pools1; max600s; firststart2026-10-20T22:10:08.262289+00:00,fixedexpiry2026-10-20T22:20:08.262289+00:00; original731201536/726040557baseline and failedcosts retained, PF07coord/binaries countatzero. No render/report/PDF/test/audit/CI/recipeinstall/physics. Grant/configIMMUTABLE; actualadmission/stdoutJSON/binaryhash/exit/cleanup in separatecontrolreceipts. Existing RootNONAUTHORQ actual22pins follows realobservation; no unchangedneutralprobe/extraACK or duplicateRootgrant/cachejob. ReturnedqueueID isnot consumption or nativeadmission.
Actualnativequeue result Queued message 01a15b4c-0b70-7c01-ab1f-38d6736c1b54 for thread 01a1575b-8982-7403-90e4-61e6c0f7d4b0. . Actualgrant/config/body/source/caps alreadybound. Root existingcompletiontask please adoptthissingleallocation and routeactualobservation to SAME NONAUTHORQ; do not createcompetingresourcebody/cachejob. Actualconsumption/PIDstart/admission/exit remainunverified untilrealcontrolreceipts.


## 2026-10-20T21:54:22.576543+00:00 | 4ff observation actually terminal
Grant4b25/config42ed SPENT_SUCCESS at21:51:50; CPU6returnedonlyexistingJarambitsharedpool. No sibling/foreignreservationrelease or reset. Actualcaps/terminal /home/ubuntu/xbabe2_kicker/captains-20261017/JARAMBIT_PAPER_V4_REVIEW_20261020/ADMIRAL_ACTUAL_4FF_OBSERVATION_EXIT_ZERO_AND_EXISTING_ROOT_Q_ACTION.json. No rerun; newgeneration afterexistingRootnonauthorpinadmission/currentbody/sourcegrant.

2026-10-20T22:28:44.336320+00:00 ADMIRAL resource state: four-host read-only22:21 results /home/ubuntu/xbabe2_kicker/captains-20261017/JARAMBIT_PAPER_V4_REVIEW_20261020/admiral-four-host-readback-20261020T222106Z/SUMMARY.json SHA b0ad2eadfdcd9121b4bdf21ed3b925a6ba6d687e9c1fae1905586aa90360e621. xbabe3 storage hold and Darwin no-Linux parity retained; no probe jobs/config changes. Joil E26 original22:19:32 expiry has passed with both exact own units already terminal22:09; no renewal or new compute. Preserve0.906GBcharged outputs, all guard/compiler failures and isolatedGit custody issue; no cleanup. No other resource reservation released. Jarambit Root own-final scan7a5 bounded adoption is separately recorded, old fixed23:57expiry unchanged.

2026-10-21T13:10:18.844241+00:00 JopeDime exactADMIN d69c853aa95a69c5894c86380937d520 logical28,29/sharednonexclusive only; siblings92,93 checked/notreserved; original120s expiry 2026-10-21T13:12:18.843343+00:00; no model/native/GPU. /home/ubuntu/xbabe2_kicker/captains-20261017/CODEX_SOL_RESTART_20261020/fleet-cadence-20261021T1252-68f5/jope-actual-provision-only/ACTUAL_FINITE_PROVISION_ONLY_OWNER_DECISION.json

2026-10-21T13:11:05.577867+00:00 JopeADMIN d69c terminal exit78/BFS09/MainPID0; logical28,29 returned to existingJopepool only, no sibling/foreignlease release; spentnonce retained. /home/ubuntu/xbabe2_kicker/captains-20261017/CODEX_SOL_RESTART_20261020/fleet-cadence-20261021T1252-68f5/jope-actual-provision-only/ACTUAL_TERMINAL_BFS09_REFUSAL_AND_RESOURCE_RETURN.json

2026-10-21T14:13:00.345311+00:00 C88 exactownADMINnonce24d CPU28,29 nonexclusive logical reservation through2026-10-21T14:15:00.344374+00:00; unchanged2CPU1.5GiBswap0, alloldcosts, NOmodel/GPU/native. /home/ubuntu/xbabe2_kicker/captains-20261017/CODEX_SOL_RESTART_20261020/fleet-cadence-20261021T1344-cbb1/jope-actual-diagnostic-provision-only/ACTUAL_FINITE_DIAGNOSTIC_OWNER_DECISION.json

2026-10-21T14:14:39.262452+00:00 C88 nonce24d CPU28,29 reservation RELEASED afteractualstrictrefusal/verifiedemptyownslice; original14:15:00.344374Z expiryretained, SPENT/noretry. /home/ubuntu/xbabe2_kicker/captains-20261017/CODEX_SOL_RESTART_20261020/fleet-cadence-20261021T1344-cbb1/jope-actual-diagnostic-provision-only/ACTUAL_TERMINAL_BFS09_CAPACITY_VALUES_AND_RESOURCE_RETURN.json SHA256 e86592b6c3b2bd7add6c45a3791babb8c3bb8f06c7f13cb1de32e1b1248a6a96

2026-10-21T14:33:34.648513+00:00 C88 correctedTmpFS 450452c8ef7aecc50704ef4a998cf29c reserveslogical28,29nonexclusive through2026-10-21T14:35:34.647203+00:00 ONEADMINcomponentonly/noGPU/native. /home/ubuntu/xbabe2_kicker/captains-20261017/CODEX_SOL_RESTART_20261020/fleet-cadence-20261021T1344-cbb1/jope-corrected-private-tmpfs-one-attempt/ACTUAL_FINITE_CORRECTED_PROPERTY_OWNER_DECISION.json

2026-10-21T14:36:00.821207+00:00 C88 450452c8ef7aecc50704ef4a998cf29c logical28,29RELEASED afteractualterminal78/verifiedownempty; originalexpiry2026-10-21T14:35:34.647203+00:00 unchanged/SPENT/no retry. /home/ubuntu/xbabe2_kicker/captains-20261017/CODEX_SOL_RESTART_20261020/fleet-cadence-20261021T1344-cbb1/jope-corrected-private-tmpfs-one-attempt/ACTUAL_TERMINAL_MOUNT_PHASE_REFUSAL_AND_RESOURCE_RETURN.json

2026-10-21T14:56:33.652850+00:00 C88 mount-diagnostic47d8 reserveslogical28,29nonexclusive through2026-10-21T14:58:33.650625+00:00 forONEADMINstageonly/noGPU/native. /home/ubuntu/xbabe2_kicker/captains-20261017/CODEX_SOL_RESTART_20261020/fleet-cadence-20261021T1344-cbb1/jope-actual-mount-diagnostic-one-attempt/ACTUAL_FINITE_MOUNT_DIAGNOSTIC_OWNER_DECISION.json


2026-10-21T15:10:23.214302+00:00 C88 nonce47d8 exact logical28,29 shared own reservation RELEASE recorded from actual14:57:31.684206 empty retirement, before original14:58:33.650625 expiry. MainPID0, own slice inactive; failed service/image retained, nonceSPENT/no retry. No sibling/foreign reservation or GPU/native grant. /home/ubuntu/xbabe2_kicker/captains-20261017/CODEX_SOL_RESTART_20261020/fleet-cadence-20261021T1344-cbb1/jope-actual-mount-diagnostic-one-attempt/ACTUAL_TERMINAL_MOUNT_REFUSAL_AND_RESOURCE_RETURN.json SHA256 15ccffa17692cca05ceb4cd3401c8cc82b86c5f333b72b5c0e0624bb7dc872e7


2026-10-21T15:31:03.531568+00:00 C89 exactnonce746a9b8540fd6f2625ce624ddb3fd67a reserveslogical28,29NONEXCLUSIVE ownADMIN only through2026-10-21T15:33:03.531568+00:00, 2CPU1.5GiBswap0/capsatcreation/100s+1s; alloldcosts retained, noGPU/native. /home/ubuntu/xbabe2_kicker/captains-20261017/CODEX_SOL_RESTART_20261020/fleet-cadence-20261021T1518-d985/jope-actual-caller-diagnostic-one-attempt/ACTUAL_FINITE_CALLER_DIAGNOSTIC_OWNER_DECISION.json


2026-10-21T15:35:43.005981+00:00 C89 nonce746a logical28,29 own NONEXCLUSIVE reservation RETURNED: actualemptyretirement15:32:35.346845 before original15:33:03.531568expiry; MainPID0/cgroupempty/sliceinactive, failedservice/imagekept. NonceSPENT, no retry/sibling/foreignreservationrelease. /home/ubuntu/xbabe2_kicker/captains-20261017/CODEX_SOL_RESTART_20261020/fleet-cadence-20261021T1518-d985/jope-actual-caller-diagnostic-one-attempt/ACTUAL_EXACT_UNIQUE_MOUNT_GUARD_REFUSAL_AND_RESOURCE_RETURN.json SHAc7aa901d8ff78f233dfcb8e0503775c7a2ece544b18c3c3f066c05a9e2827934


2026-10-21T15:45:54.305057+00:00 SC-DEC-C89-JOIL-E62-1515 reserveslogical36,37 NONEXCLUSIVE for existingRootONEsoftwarecompilerattempt, guard36/20%512MiB+payload36,37/180%11.5GiB. Firststart2026-10-21T16:15:54.305057+00:00, fixedexpiry2026-10-21T17:15:54.305057+00:00, max5400/noGPU/swap0/nice10. FOURroot8GiBallocated+apparent cumulative/7GiBmonstop/alloldcosts retained. /home/ubuntu/xbabe2_kicker/captains-20261017/resource-allocations/JOIL-REGISTERED-HYDROSTATIC-1515-XBABE2-20261021T154554Z.json SHA18e9cd65f28079154e10030d64e378ae6faf554a658f4bd526205ef0c466e5e0. Reservedunconsumed; mandatoryactualpreflight/kernel/fleetadmission. No sibling/foreign release or outside science authority.

2026-10-21T16:12:07.770002+00:00 C90 exactnonce4e1b logical28,29NONEXCLUSIVE own ADMIN only through2026-10-21T16:14:07.770002+00:00; unchanged2CPU1.5GiBswap0/capsatcreation100+1s/16MiB; noGPU/native. /home/ubuntu/xbabe2_kicker/captains-20261017/CODEX_SOL_RESTART_20261020/fleet-cadence-20261021T1606-cbc8/jope-visible-mount-one-admin-attempt/ACTUAL_FINITE_VISIBLE_MOUNT_OWNER_DECISION.json

2026-10-21T16:13:56.204148+00:00 C90 nonce4e1b ownlogical28,29NONEXCLUSIVE reservationRETURNED fromactualemptyretirement16:13:11.332347 before original16:14:07.770002; no loop/activeownunit. Errors0 intentionalexit78milestone/no native grant; allcosts/SPENThistoryretained. /home/ubuntu/xbabe2_kicker/captains-20261017/CODEX_SOL_RESTART_20261020/fleet-cadence-20261021T1606-cbc8/jope-visible-mount-one-admin-attempt/ACTUAL_ZERO_ERROR_PROVISION_MILESTONE_AND_RESOURCE_RETURN.json

2026-10-21T16:23:56.180925+00:00 C90 ONEexplicit1515 same-expiry unused-resource replacement CPU36,37NONEXCLUSIVE/total200%/12GiBswap0/nice10/noGPU. Firststart2026-10-21T16:53:56.180925+00:00; ORIGINAL fixedexpiry2026-10-21T17:15:54.305057+00:00 unchanged. Allcaps/costs/source/runtimeadmission identical; no Rootstartobserved. /home/ubuntu/xbabe2_kicker/captains-20261017/resource-allocations/JOIL-REGISTERED-HYDROSTATIC-1515-XBABE2-20261021T162356Z-EXPLICIT-SAME-EXPIRY.json SHAd93ef7a860ab418c211c1aa359d4cbea17c0ce0aa1a60a918f78ec75f508e335

2026-10-21T16:40:59.345735+00:00 C91 exact1515old18e9/d93eownlogical36,37NONEXCLUSIVE reservationRETURNED/SUPERSEDEDUNLAUNCHED afteractual5c99sourcechange; bothunitsnotfound/MainPID0. No sibling/foreignrelease/costreset/newcompilergrant. /home/ubuntu/xbabe2_kicker/captains-20261017/CODEX_SOL_RESTART_20261020/fleet-cadence-20261021T1630-5f92/JOIL_ACTUAL_1515_UNUSED_RETIREMENT_AND_ISSUER_TIMING_CORRECTION.json

2026-10-21T16:46:36.176525+00:00 C91 NEW1624/5c exactRootcompilerreservation ownlogical36,37 NONEXCLUSIVEtotal200%/12GiBswap0/nice10/noGPU, actualcargo1max2/test2,4roots8GiBallocated+apparent/7GiBstop/alloldcosts. Firststart2026-10-21T17:01:36.176525+00:00 ORIGINALfixedexpiry2026-10-21T17:15:54.305057+00:00; fullstorage/kerneladmissionmandatory/notobserved. /home/ubuntu/xbabe2_kicker/captains-20261017/resource-allocations/JOIL-REGISTERED-WORDING-1624-XBABE2-20261021T164636Z.json SHA106ece71d0a72a63a92751432f4d2ccebec9cf937cf181c685c15e94a0408da2

2026-10-21T17:00:54.873507+00:00 C91 current1624/106ownlogical36,37NONEXCLUSIVE reservationRETURNED afteractualreadonlypreflightrefusal/unlaunched. Noactiveguard/payload/stage/FF; no foreign/siblingrelease. Currentbodyrequiresnewbaselinecontinuity/newmatchingresource decision; oldfixedexpiry17:15:54.305057 retained. /home/ubuntu/xbabe2_kicker/captains-20261017/CODEX_SOL_RESTART_20261020/fleet-cadence-20261021T1630-5f92/JOIL_ACTUAL_1624_PREFLIGHT_REFUSAL_AND_BOUNDED_CONTINUITY_SOURCE_DISPOSITION.json

112,113 admiralty admiralty-css-repro-20261021t1744 2026-10-21T17:46:11.181249+00:00 (ONE fresh finite CSS fixture reproduction from f041566; reuse project standby;2CPU8GiBswap0/nice10/max600s, no GPU/native/production; preserve all prior costs)

112,113 admiralty admiralty-css-repro-20261021t1744 2026-10-21T17:46:11.415270+00:00 RELEASED BACK TO adm_extra_1 PROJECT STANDBY; own finite CSS unit inactive/empty, all failures/costs retained; no other reservation released

112,113 admiralty admiralty-css-repro-20261021t1750 2026-10-21T17:47:50.516962+00:00 (ONE fresh finite CSS fixture reproduction from f041566; reuse project standby;2CPU8GiBswap0/nice10/max600s, no GPU/native/production; preserve all prior costs)

112,113 admiralty admiralty-css-repro-20261021t1750 2026-10-21T17:47:51.302576+00:00 RELEASED BACK TO adm_extra_1 PROJECT STANDBY; own finite CSS unit inactive/empty, all failures/costs retained; no other reservation released

112,113 admiralty admiralty-css-f041-layer-03 2026-10-21T17:50:54.317648+00:00 (ONE fresh finite CSS fixture reproduction from f041566; reuse project standby;2CPU8GiBswap0/nice10/max600s, no GPU/native/production; preserve all prior costs)

112,113 admiralty admiralty-css-f041-layer-03 2026-10-21T17:51:54.662624+00:00 RELEASED BACK TO adm_extra_1 PROJECT STANDBY; own finite CSS unit inactive/empty, all failures/costs retained; no other reservation released

112,113 admiralty admiralty-css-f041-layer-04 2026-10-21T17:57:54.674734+00:00 (ONE fresh finite CSS fixture reproduction from f041566; reuse project standby;2CPU8GiBswap0/nice10/max300s/tasks1024/CPUstill2, no GPU/native/production; preserve all prior costs)

112,113 admiralty admiralty-css-f041-layer-04 2026-10-21T17:58:12.547873+00:00 RELEASED BACK TO adm_extra_1 PROJECT STANDBY; own finite CSS unit inactive/empty, all failures/costs retained; no other reservation released

112,113 admiralty admiralty-css-f041-layer-05 2026-10-21T18:03:19.912446+00:00 (ONE fresh finite CSS fixture reproduction from f041566; reuse project standby;2CPU8GiBswap0/nice10/max300s/tasks1024/CPUstill2, no GPU/native/production; preserve all prior costs)

112,113 admiralty admiralty-css-f041-layer-05 2026-10-21T18:04:07.895897+00:00 RELEASED BACK TO adm_extra_1 PROJECT STANDBY; own finite CSS unit inactive/empty, all failures/costs retained; no other reservation released

112,113 admiralty admiralty-maintenance-build-20261021T1845-8b18 2026-10-21T18:46:09.003831+00:00 (reuse adm_extra_1 project standby; ONE fresh receiving-main Linux deployment build; max2 compiler jobs/tests1/8GiB/swap0/1800s, no GPU, no foreign reservation)

112,113 admiralty admiralty-maintenance-build-20261021T1845-8b18 2026-10-21T18:52:45.704604+00:00 RELEASED BACK TO adm_extra_1 PROJECT STANDBY; exact finite own unit inactive, no other reservation released; all logs/failed bytes retained
