package localhistory

import (
	"os"
	"path/filepath"
	"testing"
)

func TestRecordListReadRestore(t *testing.T) {
	store, err := New(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	repo := t.TempDir()
	file := filepath.Join(repo, "a.txt")
	if err := os.WriteFile(file, []byte("one\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := store.Record("repo", "a.txt", "one\n", "save"); err != nil {
		t.Fatal(err)
	}
	if err := store.Record("repo", "a.txt", "one\n", "save again"); err != nil {
		t.Fatal(err)
	}
	if err := store.Record("repo", "a.txt", "two\n", "commit"); err != nil {
		t.Fatal(err)
	}
	entries, err := store.List("repo", "a.txt")
	if err != nil || len(entries) != 2 {
		t.Fatalf("entries = %+v err=%v", entries, err)
	}
	if entries[0].Reason != "commit" || entries[0].Content != "" {
		t.Fatalf("newest entry = %+v", entries[0])
	}
	old, err := store.Read("repo", "a.txt", entries[1].ID)
	if err != nil || old.Content != "one\n" {
		t.Fatalf("old entry = %+v err=%v", old, err)
	}
	if err := os.WriteFile(file, []byte("discarded\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := store.Restore("repo", repo, "a.txt", entries[1].ID); err != nil {
		t.Fatal(err)
	}
	data, _ := os.ReadFile(file)
	if string(data) != "one\n" {
		t.Fatalf("restored = %q", data)
	}
	if err := store.RecordFile("repo", repo, "a.txt", "after restore"); err != nil {
		t.Fatal(err)
	}
}

func TestPruneKeepsBound(t *testing.T) {
	store, err := New(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	for i := 0; i < 35; i++ {
		if err := store.Record("repo", "a.txt", string(rune('a'+i%26))+"\n", "save"); err != nil {
			t.Fatal(err)
		}
	}
	entries, err := store.List("repo", "a.txt")
	if err != nil || len(entries) > defaultMaxPerFile {
		t.Fatalf("entries=%d err=%v", len(entries), err)
	}
}
