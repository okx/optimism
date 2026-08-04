import 'git.just'

_EXTRALDFLAGS := if os() == "macos" { "-ldflags=-extldflags=-Wl,-ld_classic" } else { "" }

# We use both GOOS/GOARCH and TARGETOS/TARGETARCH to set the build targets.
# From the usage patterns, it looks like TARGETOS/TARGETARCH should take
# precedence if set, and default to GOOS/GOARCH if not set.
# TODO: should we just remove TARGETOS/TARGETARCH altogether eventually?
GOOS := env('GOOS', `go env GOOS`)
GOARCH := env('GOARCH', `go env GOARCH`)
TARGETOS := env('TARGETOS', GOOS)
TARGETARCH := env('TARGETARCH', GOARCH)

GORACE := "0"

_GORACE_FLAG := if GORACE == "1" { "-race " } else { "" }

# Generic escape hatch for extra Go build tags, e.g. `GOTAGS=foo just <target>`.
GOTAGS := env('GOTAGS', '')

_GOTAGS_FLAG := if GOTAGS == "" { "" } else { "-tags " + GOTAGS + " " }

# X Layer: `KMS=1 just <target>` links in KMS support, which depends on the
# private module gitlab.okg.com/okcoin-commons/ok-kms-go.
#
# That require is deliberately absent from go.mod: keeping it there made
# `go get`, `go mod download`, `go list -m all` and any downstream import of
# this repo fail for anyone without gitlab.okg.com credentials.
#
# Note that `go mod tidy` ignores build tags by design, so on a machine that can
# reach gitlab.okg.com it will want to add the require back to go.mod. Use
# `just mod-tidy`, which strips it again afterwards.
KMS := env('KMS', '0')
KMS_MODFILE := env('KMS_MODFILE', justfile_directory() / '..' / 'go.kms.mod')

_KMS_FLAG := if KMS == "1" { "-tags kms -modfile=" + KMS_MODFILE + " -mod=mod " } else { "" }

[private]
go_build BIN PKG *FLAGS:
    env GO111MODULE=on GOOS={{TARGETOS}} GOARCH={{TARGETARCH}} CGO_ENABLED=0 go build -v {{_GORACE_FLAG}}{{_GOTAGS_FLAG}}{{_KMS_FLAG}}{{FLAGS}} -o {{BIN}} {{PKG}}

[private]
go_test SELECTOR *FLAGS:
    go test -v {{_GORACE_FLAG}}{{_GOTAGS_FLAG}}{{_KMS_FLAG}}{{FLAGS}} {{SELECTOR}}

# `-parallel=1`: one fuzz worker per target, so concurrent `parallel` runs don't
# oversubscribe the CPU and starve workers into missing the `-fuzztime` deadline.
[private]
go_fuzz FUZZ TIME='10s' PKG='': (go_test PKG _EXTRALDFLAGS "-parallel" "1" "-fuzztime" TIME "-fuzz" FUZZ "-run" "NOTAREALTEST")

[private]
go_generate SELECTOR *FLAGS:
    go generate -v {{FLAGS}} {{SELECTOR}}