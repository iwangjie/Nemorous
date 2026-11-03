package main

import "testing"

func TestStatusColorSemantics(t *testing.T) {
	p := lightPalette()
	cases := []struct {
		kind string
		want string
	}{
		{"A", "green"},
		{"M", "blue"},
		{"D", "muted"},
		{"R", "purple"},
		{"U", "danger"},
		{"?", "danger"},
	}
	for _, tc := range cases {
		got := statusTextColor(tc.kind, p)
		switch tc.want {
		case "green":
			if got != p.Green {
				t.Fatalf("%s should be green", tc.kind)
			}
		case "blue":
			if got != p.Blue {
				t.Fatalf("%s should be blue", tc.kind)
			}
		case "muted":
			if got != p.TextMuted {
				t.Fatalf("%s should be the muted/deleted color", tc.kind)
			}
		case "purple":
			if got != p.Purple {
				t.Fatalf("%s should be purple", tc.kind)
			}
		case "danger":
			if got != p.Danger {
				t.Fatalf("%s should be danger", tc.kind)
			}
		}
	}
}
