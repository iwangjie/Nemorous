package main

import (
	"strings"
	"testing"
	"time"
)

func TestBlameInlayTextFitsAvailableColumns(t *testing.T) {
	label := "Administrator · 2025-11-19 · b3edda1b · Initial commit with a long subject"
	line := strings.Repeat("code", 10)
	got := blameInlayText(label, line, 800, 14)
	if got == "" {
		t.Fatal("expected a shortened annotation")
	}
	if strings.Contains(got, "\n") {
		t.Fatalf("annotation wraps: %q", got)
	}
	if !strings.HasSuffix(got, "…") {
		t.Fatalf("expected an ellipsis, got %q", got)
	}
	if len([]rune(got)) >= len([]rune(label)) {
		t.Fatalf("annotation was not shortened: %q", got)
	}
}

func TestBlameInlayTextStaysQuietWhenLineFillsWidth(t *testing.T) {
	if got := blameInlayText("author · 2026-01-01 · abc12345", strings.Repeat("x", 200), 420, 14); got != "" {
		t.Fatalf("expected no annotation without room, got %q", got)
	}
}

func TestBlameRecencyColor(t *testing.T) {
	p := lightPalette()
	now := time.Date(2026, 10, 30, 12, 0, 0, 0, time.Local)
	if got := blameRecencyColor(now.Add(-2*24*time.Hour).Unix(), now, p); got != p.Accent {
		t.Fatalf("recent commit should use accent, got %v", got)
	}
	if got := blameRecencyColor(now.Add(-14*24*time.Hour).Unix(), now, p); got != p.Amber {
		t.Fatalf("month-old commit should use amber, got %v", got)
	}
	if got := blameRecencyColor(now.Add(-60*24*time.Hour).Unix(), now, p); got != p.TextMuted {
		t.Fatalf("old commit should be muted, got %v", got)
	}
}
