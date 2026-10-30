package config

// RunConfig is one Run/Debug/Test configuration. The runner lives in
// internal/run; this type is its persisted form.
type RunConfig struct {
	ID          string            `json:"id"`
	Name        string            `json:"name"`
	Kind        string            `json:"kind"` // run, test, debug
	Command     string            `json:"command"`
	Args        string            `json:"args,omitempty"`
	Cwd         string            `json:"cwd,omitempty"`
	Env         map[string]string `json:"env,omitempty"`
	Adapter     string            `json:"adapter,omitempty"`
	AdapterArgs string            `json:"adapter_args,omitempty"`
	Network     string            `json:"network,omitempty"` // stdio or tcp
	Address     string            `json:"address,omitempty"` // tcp host:port
	Program     string            `json:"program,omitempty"`
	ProgramArgs string            `json:"program_args,omitempty"`
	Launch      string            `json:"launch,omitempty"` // JSON merged into DAP launch arguments
}
