#set document(title: "Josh Heinrichs — Resume", author: "Josh Heinrichs")
#set page(paper: "us-letter", margin: (x: 0.75in, y: 0.6in), footer: context {
  let out = sys.inputs.at("out", default: none)
  if out == none or here().page() != counter(page).final().first() { return }
  align(right, text(font: "Source Code Pro", size: 6.5pt, fill: luma(150), out))
})
#set text(font: "Source Serif 4", size: 10pt, lang: "en", number-type: "lining", hyphenate: false)
#set par(justify: false, leading: 0.62em, spacing: 0.9em)
#set list(indent: 0.4em, body-indent: 0.5em, spacing: 0.66em, marker: ([•], [–]))

#let accent = rgb("#1f3a5f")

#show link: it => text(fill: accent, underline(offset: 2pt, stroke: 0.5pt + accent.lighten(50%), it))

#show heading.where(level: 1): it => block(above: 1.5em, below: 0.9em, width: 100%, sticky: true, {
  set text(size: 13pt, weight: "regular", fill: accent)
  smallcaps(it.body)
  v(-0.55em)
  line(length: 100%, stroke: 0.5pt + accent)
})

#let entry(org, place, roles, body) = block(above: 1.5em, below: 0pt, breakable: false, {
  grid(
    columns: (1fr, auto),
    align: (left, right),
    row-gutter: 0.55em,
    strong(org), strong(place),
    ..roles.map(((title, dates)) => (emph(title), emph(dates))).flatten(),
  )
  v(0.2em)
  body
})

#let skills(..rows) = grid(
  columns: (auto, 1fr),
  column-gutter: 1.2em,
  row-gutter: 0.6em,
  ..rows.pos().map(((name, body)) => (strong(name), body)).flatten(),
)

#let project(name, year, tech, body) = block(above: 1.3em, below: 0pt, breakable: false, {
  grid(
    columns: (1fr, auto),
    align: (left, right),
    [#strong(name) #h(0.4em) #emph(tech)], emph(year),
  )
  v(0.2em)
  body
})

#grid(
  columns: (1fr, auto),
  align: (left + bottom, right + bottom),
  [
    #text(size: 24pt)[Josh Heinrichs] \
    #text(size: 11.5pt, fill: accent)[Senior Engineer, Developer Productivity]
  ],
  [
    #link("mailto:joshiheinrichs@gmail.com") \
    #link("https://github.com/joshheinrichs")[github.com/joshheinrichs]
  ],
)

= Experience

#entry("Shopify", "Remote", (
  ("Senior Engineer, Developer Productivity", "Feb. 2022 – Present"),
))[
  Building development environments and build systems for humans and AI agents,
  and championing the move to Nix in production.

  - Built Shopify’s Nix-based development environments and led the migration to
    them. They now cover the vast majority of development and save tens of
    thousands of developer hours per year.
    - Now powering instant development environments for
      #link("https://shopify.engineering/under-the-river")[River], Shopify’s
      internal coding agent.
    - Strengthened reproducibility and supply chain security.
  - Invented a novel Nix evaluation caching technique, cutting evaluation times by
    over 99% and saving millions of machine hours per year in Shopify’s build farm
    and tens of thousands of developer hours locally.
  - Designed and built Ruby support for
    #link("https://www.youtube.com/watch?v=xgH5xKXvaH0")[Tectonix], the Nix-based
    system behind Shopify’s monorepo.
    - Made Bundler transparently install gems through Nix, so developers kept
      their workflows and tools.
    - Added monorepo support to Bundler in partnership with the Ruby team.
    - Guiding integrations for other ecosystems: Go, Rust, Python (uv),
      JavaScript (pnpm), and the JVM (Gradle).
  - Patched Git for Shopify’s monorepo, making key commands such as status,
    checkout, and add 10–100× faster and saving hundreds of thousands of hours per
    year.
  - Devised and led a redesign of Spin, Shopify’s cloud development environment
    platform, cutting its infrastructure costs by over 80% while substantially
    improving performance.
    - Moved infrastructure from Kubernetes to virtual machines.
    - Added auto-suspension, zram memory compression, and compressed, auto‑resizing
      btrfs disks.
    - Hardened security and added support for GPUs, io_uring, and nested
      containers.
]

#entry("Siemens EDA", "Saskatoon", (
  ("Software Verification Lead", "Dec. 2017 – Feb. 2022"),
  ("Release Manager", "May 2017 – Dec. 2017"),
  ("Software Engineer", "May 2016 – May 2017"),
))[
  Worked on Variation Designer and Solido Design Environment (SDE), circuit
  analysis tools for custom IC design, improving developer productivity,
  infrastructure efficiency, and software quality.

  - Led an 8-person verification team for four years while owning DevOps for the
    product, from test and build infrastructure to releases.
  - Architected a custom distributed test orchestrator, cutting test runs 8× from
    4 hours to under 30 minutes.
  - Designed and built a cascading merge queue on Jenkins that merged changes
    forward across a large monorepo’s release branches, serving multiple teams
    with independent release cycles.
  - Managed a high-performance computing cluster of several thousand nodes,
    adding monitoring and analytics and fixing a kernel issue to make disk
    access over 40× faster.
  - Oversaw releases of Variation Designer, used by many of the world’s largest
    semiconductor companies, and overhauled development processes to ship more
    often with higher quality.
  - Added ARM64 support to the build and test infrastructure.
  - Led the adoption of Docker in test infrastructure, improving reliability and
    broadening environment coverage.
]

#entry("University of Saskatchewan", "Saskatoon", (
  ("Undergraduate Researcher – Algorithms", "May 2015 – Aug. 2015"),
  ("Undergraduate Researcher – Software Engineering", "May 2014 – Aug. 2014"),
))[
  - Built a Java application for creating and analyzing Delaunay
    triangulations, and studied properties of restricted Delaunay
    triangulations.
  - Improved the quality of a large Java health model from Columbia University
    by adding version control, continuous integration, documentation, and tests.
]

= Talks

#project(
  link("https://media.ccc.de/v/nixcon2025-56413-nix-based-development-e")[Nix-based development environments at Shopify (reprise)],
  "Sept. 2025",
  "NixCon 2025",
)[
  How Shopify moved its development environments to Nix: why an earlier attempt
  stalled, how projects migrated incrementally, and what we learned supporting a
  polyglot organization.
]

= Projects

#project(
  link("https://github.com/joshheinrichs/apiary")[Apiary],
  "2026",
  "Nix, Rust",
)[
  My home infrastructure in a single Nix monorepo, from my workstation to the
  firmware in my espresso machine, e‑reader, and synthesizer.

  - Built a sandbox for desktop apps such as Steam, Discord, and Spotify that
    isolates the filesystem, network, GPU, Wayland, and D-Bus, with per‑app
    network zones.
  - Set up system-wide continuous profiling with eBPF, which surfaced resource
    leaks in Firefox’s hardware video decoding that I then patched.
  - Multiplexed two Wayland compositors on one GPU, so a single computer drives
    separate desk and TV setups.
]

#project(link("https://github.com/th-truong/tj-chess")[TJ Chess], "2021", "Python, PyTorch")[
  Built a neural network chess engine with a fierce early game.
]
#project(link("https://github.com/joshheinrichs/geosource")[GeoSource], "2016", "Go, JavaScript, HTML, PostgreSQL")[
  Built a web app for collecting, exploring, and discussing photos and other
  data on a map, as my university honours project.
]
#project(link("https://github.com/joshheinrichs/mumble-jukebox")[Mumble Jukebox], "2015", "Go")[
  Built a VoIP bot that streamed music and took commands from chat.
]

= Skills

#skills(
  ("Languages", [Python, Rust, Go, Ruby, Bash, C, C++]),
  ("Build & packaging", [Nix, Make, CMake, Bundler, uv, pnpm, Gradle, Docker]),
  ("Source control & CI", [Git (internals, monorepo scaling), Jenkins, merge queues]),
  ("Linux internals", [namespaces, cgroups, io_uring, zram, btrfs, FUSE]),
  ("Performance & observability", [eBPF, strace, Pyroscope, Grafana, Prometheus, OpenTelemetry, Alloy, Jupyter]),
  ("Infrastructure", [Google Cloud Platform (Compute Engine), Kubernetes, SGE, LSF]),
)

= Education

#entry("University of Saskatchewan", "Saskatoon", (
  ("B.Sc. Honours Software Engineering", "Sept. 2012 – April 2016"),
))[
  Graduated with high honours, focusing on algorithms, programming languages,
  concurrency, and networking.
]

#let max-pages = 2

#context if counter(page).final().first() > max-pages {
  panic("resume overflows onto " + str(counter(page).final().first()) + " pages")
}
