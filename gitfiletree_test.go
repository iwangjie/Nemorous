package main

import (
	"testing"

	"github.com/iwangjie/nemorous/internal/gitops"
)

func TestBuildGitCommitTreeCompactsDirectories(t *testing.T) {
	root := buildGitCommitTree([]gitops.CommitFile{
		{Path: "sms-service/src/main/Java/A.java", Status: "M"},
		{Path: "sms-service/src/main/Java/B.java", Status: "A"},
		{Path: "README.md", Status: "M"},
	})
	if len(root.files) != 1 || root.files[0].Path != "README.md" {
		t.Fatalf("root files = %+v", root.files)
	}
	if len(root.dirs) != 1 || root.dirs[0].name != "sms-service/src/main/Java" {
		t.Fatalf("compacted dirs = %+v", root.dirs)
	}
	if got := gitCommitTreeFileCount(root.dirs[0]); got != 2 {
		t.Fatalf("dir file count = %d", got)
	}
}
