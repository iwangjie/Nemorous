package conflict

import "testing"

func TestMergeHunkNonOverlappingLines(t *testing.T) {
	h := Hunk{
		HasBase: true,
		Base:    "a\nb\nc\nd\n",
		Ours:    "a\nB\nc\nd\n",
		Theirs:  "a\nb\nc\nD\n",
	}
	got, ok := mergeHunkText(h)
	if !ok {
		t.Fatal("disjoint changes should merge")
	}
	if got != "a\nB\nc\nD\n" {
		t.Fatalf("merged = %q", got)
	}
}

func TestMergeHunkOverlapStaysConflicted(t *testing.T) {
	h := Hunk{
		HasBase: true,
		Base:    "a\nb\nc\n",
		Ours:    "a\nX\nc\n",
		Theirs:  "a\nY\nc\n",
	}
	if _, ok := mergeHunkText(h); ok {
		t.Fatal("overlapping changes must stay conflicted")
	}
}

func TestMergeHunkIdenticalInsertions(t *testing.T) {
	h := Hunk{
		HasBase: true,
		Base:    "a\nb\n",
		Ours:    "a\nX\nb\n",
		Theirs:  "a\nX\nb\n",
	}
	got, ok := mergeHunkText(h)
	if !ok || got != "a\nX\nb\n" {
		t.Fatalf("merged = %q ok=%v", got, ok)
	}
}

func TestSmartResolveMergesDisjointHunks(t *testing.T) {
	doc, err := Parse("<<<<<<< HEAD\na\nB\nc\nd\n||||||| base\na\nb\nc\nd\n=======\na\nb\nc\nD\n>>>>>>> other\n")
	if err != nil {
		t.Fatal(err)
	}
	out, resolved, remaining := doc.SmartResolve()
	if resolved != 1 || remaining != 0 || out != "a\nB\nc\nD\n" {
		t.Fatalf("out=%q resolved=%d remaining=%d", out, resolved, remaining)
	}
}
