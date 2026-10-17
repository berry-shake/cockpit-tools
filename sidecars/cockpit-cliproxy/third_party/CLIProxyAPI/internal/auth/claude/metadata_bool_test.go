package claude

import "testing"

func TestReadMetadataBool(t *testing.T) {
	if ReadMetadataBool(nil, "flag") {
		t.Fatal("nil metadata pointer must return false")
	}
	for _, tc := range []struct {
		name     string
		metadata map[string]any
		want     bool
	}{
		{name: "nil map"},
		{name: "missing", metadata: map[string]any{}},
		{name: "true", metadata: map[string]any{"flag": true}, want: true},
		{name: "false", metadata: map[string]any{"flag": false}},
		{name: "string is not boolean", metadata: map[string]any{"flag": "true"}},
		{name: "number is not boolean", metadata: map[string]any{"flag": 1}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := ReadMetadataBool(&tc.metadata, "flag"); got != tc.want {
				t.Fatalf("ReadMetadataBool() = %v, want %v", got, tc.want)
			}
		})
	}
}
