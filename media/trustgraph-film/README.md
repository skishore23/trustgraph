# Permission follows the work

An original 1 minute 58 second TrustGraph architecture film. Hand-drawn collage,
vector motion, synthetic narration and original instrumental score. It follows
one bank's patch through a confidential-source block, fresh-generation repair,
exact approval, real publication, Hydra evidence and policy experiments.

[Play the 1080p film](deliverables/TrustGraph-Architecture.mp4) ·
[Subtitles](deliverables/TrustGraph.srt) · [Screenplay](SCRIPT.md)

The Reddit reference supplied by the user informed the broad editorial collage
and JavaScript motion direction. Its post described that style; its complete
video could not be inspected. No artwork, script, music or animation was copied.

## Production

- Comfy CLI generated three original illustrations and eight original narration
  stems using template workflows. Jobs and decisions are in `job_ids.txt` and
  append-only `LOG.md`; prompt variants are in `workflows/`.
- `produce.py` creates template-slot variants, submits with the authenticated
  Comfy CLI and downloads completed jobs. It never embeds API keys.
- `render.mjs` draws original 1920x1080 animation at 24fps with native Canvas and
  streams frames to ffmpeg. Geometry, timing and labels are deterministic.
- `score.mjs` composes three instrumental cues with no external audio samples.
  Cloud music attempts failed, timed out or arrived too late; unused output is
  not represented as part of the final mix.
- `mix.py` conforms the Comfy narration to `timeline.json`, ducks score under
  speech, normalizes loudness and muxes selectable subtitles.

```sh
npm ci --prefix media/trustgraph-film
python3 media/trustgraph-film/produce.py watch
node media/trustgraph-film/render.mjs
node media/trustgraph-film/score.mjs
python3 media/trustgraph-film/mix.py
```

Requires signed-in `comfy`, ffmpeg/ffprobe, Node and macOS Georgia/Menlo fonts.
Generated cloud assets and runtime references are ignored in Git; job IDs allow
recovery from the same Comfy account. The final MP4, poster and subtitles are
included. `qc/QC_LOG.md` records visual and audio checks; measured final mix is
-14.42 LUFS / -1.18 dBTP. Subtitles use sentence-level timing, not forced alignment.

The film is an architecture animation. Controlled bank documents and explicitly
configured audiences support the live integration; imported enterprise IAM and
Composio are not claimed. See the repository's live qualification for actual
Codex, HydraDB and GitHub evidence.
