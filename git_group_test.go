package main

import (
	"strings"
	"testing"
	"time"
)

func TestGitDayGroupingLabels(t *testing.T) {
	now := time.Now()
	if got := gitDayKey(now.Unix()); got != now.Format("2006-01-02") {
		t.Fatalf("day key = %q", got)
	}
	if got := gitDayLabel(now.Unix()); !strings.HasPrefix(got, "今天") {
		t.Fatalf("today label = %q", got)
	}
	yesterday := now.AddDate(0, 0, -1)
	if got := gitDayLabel(yesterday.Unix()); !strings.HasPrefix(got, "昨天") {
		t.Fatalf("yesterday label = %q", got)
	}
	if got := gitDayKey(0); got != "" {
		t.Fatalf("zero time key = %q", got)
	}
}
