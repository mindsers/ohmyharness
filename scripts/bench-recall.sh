#!/usr/bin/env bash
# Measure retrieval, instead of arguing about it.
#
# Builds a corpus and a question set out of this repo's own git history, asks
# both retrieval engines every question, and counts how often each puts the
# right note first.
#
# The point is that neither side is authored by whoever wrote the ranker: a
# commit BODY becomes a note (it already reads as "we expected X, we got Y"),
# and that commit's SUBJECT becomes the query. Same fact, different words —
# which is what a half-remembered question is. Nobody picks the questions and
# nobody picks the answers, so the result cannot be tilted.
#
#   ./scripts/bench-recall.sh                  omh only
#   ./scripts/bench-recall.sh --with-iwe       omh vs iwe (needs docker)
#   ./scripts/bench-recall.sh --answers DIR    score a corpus whose `## Answers`
#                                              somebody else filled in
#
# Requires: cargo build first, python3, and git.
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OMH="$REPO/target/debug/omh"
# Pinned, so the corpus does not drift as the repo grows and two runs a month
# apart remain comparable.
REF="${BENCH_REF:-1f351a4}"
IWE_VERSION="0.19.0"
OUT="${BENCH_OUT:-$REPO/target/bench}"

WITH_IWE=0
ANSWERS_DIR=""
while [ $# -gt 0 ]; do
  case "$1" in
    --with-iwe) WITH_IWE=1; shift ;;
    --answers)  ANSWERS_DIR="$2"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

[ -x "$OMH" ] || { echo "build first: cargo build" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }
git -C "$REPO" rev-parse --verify -q "$REF" >/dev/null \
  || { echo "no such ref: $REF (set BENCH_REF)" >&2; exit 1; }

mkdir -p "$OUT"
CORPUS="${ANSWERS_DIR:-$OUT/notes}"

if [ -z "$ANSWERS_DIR" ]; then
  rm -rf "$OUT/notes"
  mkdir -p "$OUT/notes"
  REPO="$REPO" REF="$REF" OUT="$OUT" python3 - <<'PY'
import subprocess, json, os, re
repo, ref, out = os.environ['REPO'], os.environ['REF'], os.environ['OUT']
def git(*a):
    return subprocess.run(['git','-C',repo,*a],capture_output=True,text=True).stdout
def slug(s):
    o=[]
    for ch in s.lower():
        if ch.isalnum(): o.append(ch)
        elif not o or o[-1]!='-': o.append('-')
    return ''.join(o).strip('-')
qs, seen = [], set()
for h in git('log','--format=%H',ref).split():
    subj = git('log','-1','--format=%s',h).strip()
    body = git('log','-1','--format=%b',h).strip()
    paras = [p.strip().replace('\n',' ') for p in body.split('\n\n') if p.strip()]
    if len(' '.join(paras).split()) < 60:   # no body worth turning into a note
        continue
    # The key comes from what was OBSERVED, exactly as `remember` derives it.
    # The subject never appears in the note, so searching with it is a real
    # paraphrase rather than a lookup.
    first = re.split(r'(?<=[.!?])\s', paras[0])[0]
    key = 'surprise/' + slug(first)[:60].rstrip('-')
    if key in seen: continue
    seen.add(key)
    note = (f"---\nkey: {key}\ntype: surprise\nsource: session s01, claude\n"
            f"recorded: 2026-07-01\n---\n\n# {first}\n\n"
            f"## Expected\n{paras[0]}\n\n"
            f"## Observed\n{paras[1] if len(paras)>1 else paras[0]}\n\n"
            f"## Evidence\n{' '.join(paras[2:])[:1200] or 'see the commit'}\n\n"
            # Left EMPTY on purpose. Whoever writes these must not be whoever
            # wrote the ranker; see docs/design/memory-benchmark.md.
            f"## Answers\n\n")
    p = os.path.join(out,'notes',key+'.md')
    os.makedirs(os.path.dirname(p),exist_ok=True)
    open(p,'w').write(note)
    qs.append({'query':subj,'answer':key})
json.dump(qs, open(os.path.join(out,'questions.json'),'w'), indent=1)
open(os.path.join(out,'queries.txt'),'w').write('\n'.join(q['query'] for q in qs)+'\n')
print(f"corpus: {len(qs)} notes and {len(qs)} queries, from {ref}")
PY
else
  echo "corpus: $ANSWERS_DIR (supplied)"
  [ -f "$OUT/questions.json" ] || { echo "run once without --answers first" >&2; exit 1; }
fi

python3 - "$OUT" <<'PY'
import json,sys
out=sys.argv[1]
qs=json.load(open(out+'/questions.json'))
with open(out+'/req.jsonl','w') as f:
    for i,q in enumerate(qs):
        f.write(json.dumps({"jsonrpc":"2.0","id":i,"method":"tools/call",
            "params":{"name":"recall","arguments":{"question":q['query']}}})+'\n')
PY

# How many notes actually declare their questions. Reported every run: a corpus
# nobody has filled in scores exactly like one with no `## Answers` at all, and
# that must not be mistakable for the experiment having been run.
CORPUS="$CORPUS" python3 - <<'PY'
import os, glob
corpus = os.environ['CORPUS']
notes = glob.glob(os.path.join(corpus, '**', '*.md'), recursive=True)
filled = 0
for p in notes:
    body = open(p).read()
    if '## Answers' in body and body.split('## Answers', 1)[1].strip():
        filled += 1
if filled == 0:
    print(f"  ! 0/{len(notes)} notes declare any questions - this is the BASELINE,")
    print("    not the write-side experiment. Fill `## Answers` first;")
    print("    see scripts/answers-prompt.md.")
elif filled < len(notes):
    print(f"  ! only {filled}/{len(notes)} notes declare questions - partial corpus")
else:
    print(f"  {filled}/{len(notes)} notes declare their questions")
PY

"$OMH" memory serve --team "$CORPUS" --local "$OUT/scratch-local" \
  < "$OUT/req.jsonl" > "$OUT/omh.jsonl" 2>/dev/null

if [ "$WITH_IWE" = 1 ]; then
  if ! docker info >/dev/null 2>&1; then
    # Said out loud rather than skipped quietly: a table with one engine in it
    # must not look like a table with two.
    echo "  ! docker is not running — iwe arm SKIPPED, omh numbers only"
    WITH_IWE=0
  else
    docker build -q -t omh-bench-iwe - >/dev/null <<EOF
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/*
ARG TARGETARCH
RUN set -eu; case "\${TARGETARCH:-arm64}" in arm64) T=aarch64-unknown-linux-gnu;; *) T=x86_64-unknown-linux-gnu;; esac; \
    curl -fsSL -o /tmp/i.tgz "https://github.com/iwe-org/iwe/releases/download/iwe-v$IWE_VERSION/iwe-v$IWE_VERSION-\${T}.tar.gz" \
 && tar xzf /tmp/i.tgz -C /usr/local/bin && chmod +x /usr/local/bin/iwe*
EOF
    docker run --rm -v "$CORPUS:/notes" -w /notes omh-bench-iwe iwe init >/dev/null 2>&1
    # --lexical is BM25 over title and body. --fuzzy matches titles and keys
    # only, so it scores nothing on prose questions; lexical is iwe at its best
    # here, and giving it its best shot is the point.
    docker run --rm -v "$CORPUS:/notes" -v "$OUT:/bench" -w /notes omh-bench-iwe \
      sh -c 'while IFS= read -r q; do echo "###"; iwe find --lexical "$q" 2>/dev/null; done < /bench/queries.txt' \
      > "$OUT/iwe.txt" 2>&1
  fi
fi

OUT="$OUT" WITH_IWE="$WITH_IWE" python3 - <<'PY'
import json,re,os,math
out, with_iwe = os.environ['OUT'], os.environ['WITH_IWE']=='1'
def canon(k):
    # iwe drops the directory prefix and truncates; omh keeps the whole path.
    # Compare on the leading alphanumerics of the leaf, which both preserve.
    return re.sub(r'[^a-z0-9]','',k.split('/')[-1].lower())[:38]
qs=json.load(open(out+'/questions.json')); want=[canon(q['answer']) for q in qs]; n=len(qs)
def omh():
    got={}
    for line in open(out+'/omh.jsonl'):
        m=json.loads(line); keys=[]
        for l in m['result']['content'][0]['text'].split('\n'):
            l=l.strip().lstrip('├└─│ ')
            if not l or l.startswith('…') or l.startswith('No notes'): continue
            k=l.split('  ')[0].strip()
            if k: keys.append(canon(k))
        got[m['id']]=keys
    return [got[i] for i in range(len(got))]
def iwe():
    return [[canon(m.group(1)) for m in
             (re.match(r'\s*-\s*\[[^\]]*\]\(([^)]+)\)',l) for l in b.strip().split('\n')) if m]
            for b in open(out+'/iwe.txt').read().split('###')[1:]]
rows=[("omh recall", omh())]
if with_iwe and os.path.exists(out+'/iwe.txt'):
    rows.append(("iwe --lexical (BM25)", iwe()))
print(f"\n  {n} notes, {n} half-remembered queries\n")
print(f"  {'engine':<26} {'P@1':>7} {'top-3':>7} {'top-8':>7} {'returned':>9}")
for name,res in rows:
    f=lambda k: 100*sum(1 for x in range(n) if want[x] in res[x][:k])/n
    print(f"  {name:<26} {f(1):6.1f}% {f(3):6.1f}% {f(8):6.1f}% {sum(len(r) for r in res)/n:9.1f}")
if len(rows)==2:
    (_,a),(_,b)=rows
    aw=sum(1 for x in range(n) if want[x] in a[x][:1] and want[x] not in b[x][:1])
    bw=sum(1 for x in range(n) if want[x] in b[x][:1] and want[x] not in a[x][:1])
    d=aw+bw
    p=(sum(math.comb(d,k) for k in range(0,min(aw,bw)+1))/2**d*2) if d else 1.0
    verdict="a real difference" if p<0.05 else "indistinguishable"
    print(f"\n  P@1 disagreements: omh {aw}, iwe {bw} — McNemar p={min(p,1.0):.3f} ({verdict})")
print()
PY
