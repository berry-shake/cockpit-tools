package executor

import (
	"context"
	"fmt"
	"net/http"
	"testing"

	"github.com/router-for-me/CLIProxyAPI/v7/internal/config"
	cliproxyauth "github.com/router-for-me/CLIProxyAPI/v7/sdk/cliproxy/auth"
)

func TestCodexDirectImageHeadersKeepOfficialIdentityAndClientVersion(t *testing.T) {
	for _, apiKey := range []bool{false, true} {
		for _, explicitHeaders := range []bool{false, true} {
			for _, stream := range []bool{false, true} {
				t.Run(fmt.Sprintf("apiKey=%v/explicit=%v/stream=%v", apiKey, explicitHeaders, stream), func(t *testing.T) {
					source := map[string]string{
						"User-Agent": "downstream-client/9.9",
						"Originator": "Codex Desktop",
						"Version":    "0.135.0",
					}
					headers := make(http.Header)
					for key, value := range source {
						headers.Set(key, value)
					}
					ctx := contextWithGinHeaders(source)
					if explicitHeaders {
						ctx = context.Background()
					}
					req, err := http.NewRequestWithContext(ctx, http.MethodPost, "http://localhost/images/generations", nil)
					if err != nil {
						t.Fatal(err)
					}
					auth := &cliproxyauth.Auth{Provider: "codex", Metadata: map[string]any{"access_token": "test-token"}}
					if apiKey {
						auth.Attributes = map[string]string{"api_key": "test-token"}
					}
					cfg := &config.Config{Codex: config.CodexConfig{APIServiceCompatibility: true}}
					if explicitHeaders {
						applyCodexDirectImageHeaders(req, auth, "test-token", stream, cfg, headers)
					} else {
						applyCodexDirectImageHeaders(req, auth, "test-token", stream, cfg)
					}
					wantUA, wantOriginator := codexUserAgent, codexOriginator
					if apiKey {
						if got := req.Header.Get("Version"); got != source["Version"] {
							t.Fatalf("API-key Version = %q, want %q", got, source["Version"])
						}
					}
					if got := req.Header.Get("User-Agent"); got != wantUA {
						t.Fatalf("User-Agent = %q, want %q", got, wantUA)
					}
					if got := req.Header.Get("Originator"); got != wantOriginator {
						t.Fatalf("Originator = %q, want %q", got, wantOriginator)
					}
					if got := headers.Get("User-Agent"); got != source["User-Agent"] {
						t.Fatal("source headers were mutated")
					}
				})
			}
		}
	}
}
