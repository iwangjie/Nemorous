package config

import "testing"

func TestChangeLists(t *testing.T) {
	var cfg Config
	cfg.AssignToChangeList("登录", []string{"a::one.go", "a::two.go"})
	cfg.AssignToChangeList("支付", []string{"b::pay.go"})
	if got := cfg.ChangeListFor("a::one.go"); got != "登录" {
		t.Fatalf("list = %q", got)
	}
	cfg.AssignToChangeList("支付", []string{"a::two.go"})
	if got := cfg.ChangeListFor("a::two.go"); got != "支付" {
		t.Fatalf("moved file list = %q", got)
	}
	if len(cfg.ChangeLists[0].Files) != 1 {
		t.Fatalf("old list = %+v", cfg.ChangeLists[0])
	}
	valid := map[string]bool{"a::one.go": true, "a::two.go": true, "b::pay.go": true}
	if n := cfg.ChangeListCount("", valid); n != 0 {
		t.Fatalf("default count = %d", n)
	}
	cfg.DeleteChangeList("支付")
	if got := cfg.ChangeListFor("a::two.go"); got != "" {
		t.Fatalf("deleted list still assigned: %q", got)
	}
	if n := cfg.ChangeListCount("", valid); n != 2 {
		t.Fatalf("default count after delete = %d", n)
	}
	cfg.AssignToChangeList("登录", []string{"missing::x"})
	cfg.PruneChangeLists(valid)
	for _, list := range cfg.ChangeLists {
		for _, f := range list.Files {
			if f == "missing::x" {
				t.Fatal("prune left missing file")
			}
		}
	}
}
