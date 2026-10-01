# Benchmark field usage log

Every download and use of the benchmark warriors: the date, the run or script, the purpose and the
warriors involved with their source. `fetch.py` adds a line for each download and `breed.py` one
for each ranking and pMARS check. Development runs of the earlier GA, which played the field as
opponents, are logged below too. The log holds names and sources only, no code. The warriors themselves are never
committed, are played against only, and never reach the breeding loop or its prompts.

Sources considered and left out:

- corewar.co.uk (benchmark sets such as Wilkies and WilFiz): the site answers scripts with a bot
  check (reCAPTCHA), so it does not allow automated download. Checked 2026-10-01.

| Date | Run or script | Purpose | Warriors |
|------|---------------|---------|----------|
| 2026-10-01 | manual (curl) | check the source's terms and that every warrior assembles under `--standard pmars`; one archive download into a temporary folder | all 59 warriors of the Koenigstuhl 94nop Top-50 (https://asdflkj.net/COREWAR/koenigstuhl.html) |
| 2026-10-01 | fetch.py --repin | pin the benchmark field | all 59 warriors of field.json (Koenigstuhl 94nop Top-50, Christoph Birk, https://asdflkj.net/COREWAR/koenigstuhl.html) |
| 2026-10-01 | fetch.py | download for the local breeding benchmark | all 59 warriors of field.json (Koenigstuhl 94nop Top-50, Christoph Birk, https://asdflkj.net/COREWAR/koenigstuhl.html) |
| 2026-10-01 | smoke, iteration 1 | local breeding benchmark, opponents only | the first 5 warriors of field.json (Koenigstuhl 94nop Top-50, Christoph Birk, https://asdflkj.net/COREWAR/koenigstuhl.html) |
| 2026-10-01 | breed.py rank | rank the benchmark field | all 59 warriors of field.json (Koenigstuhl 94nop Top-50, Christoph Birk, https://asdflkj.net/COREWAR/koenigstuhl.html) |
| 2026-10-01 | breed.py rank | rank the benchmark field | all 59 warriors of field.json (Koenigstuhl 94nop Top-50, Christoph Birk, https://asdflkj.net/COREWAR/koenigstuhl.html) |
| 2026-10-01 | smoke, iteration 1 | local breeding benchmark, opponents only | all 59 warriors of field.json (Koenigstuhl 94nop Top-50, Christoph Birk, https://asdflkj.net/COREWAR/koenigstuhl.html) |
| 2026-10-01 | smoke, iteration 2 | local breeding benchmark, opponents only | all 59 warriors of field.json (Koenigstuhl 94nop Top-50, Christoph Birk, https://asdflkj.net/COREWAR/koenigstuhl.html) |
| 2026-10-01 | breed.py pmars | engine check against pMARS | Carmilla_3.red, artofcorewar.red, borg.red, burningmetal.red, excalibur.red, excapedExperiment.red, ghostmurmur.red, kingcobra.red, kusanagi3.red, maelstrom.red, metal.red, neith.red, nightstalker.red, numb.red, pendulum.red, reepicheep.red, twinstorms.red, xenosmilus.red (Koenigstuhl 94nop Top-50, Christoph Birk) |
| 2026-10-01 | breed.py pmars | engine check against pMARS | Carmilla_3.red, Eternal_Exile.red, armadillo.red, artofcorewar.red, azathoth.red, borg.red, burningmetal.red, clairvoyance.red, clrsrc.red, dofa.red, eccentric.red, elvenking2.red, excapedExperiment.red, frothfizzle.red, ghostmurmur.red, hullabaloo.red, hullabaloo3.red, kingcobra.red, kusanagi3.red, lastjudgement.red, lore2.red, lzma2.red, maelstrom.red, metal.red, neith.red, nightstalker.red, pdqscan.red, perseus.red, positiveknife.red, reepicheep.red, spiritual.red (Koenigstuhl 94nop Top-50, Christoph Birk) |
