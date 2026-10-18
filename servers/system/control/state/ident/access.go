package ident

// Access (docs/dev/joining.md, "Access"): signing up gives nothing. A
// person may use the fleet's services through an invite - the release's
// member list, or an invite the release key signed, both David's - or
// through a box of theirs that counts: enlisted, qualifying by its own
// report, and not gone 30 days by their doing. A ban is a ban: the
// release's list shutting someone out, or their own leaving, beats every
// invite and every box.

import "sort"

// BoxInfo is what the agreed state says of a box now: from its heartbeats
// (state.BoxState), the release's validators' word on when it was heard
type BoxInfo struct {
	LastSeen int64              // agreed ms; 0, never heard
	Facts    map[string]float64 // its last heartbeat's
}

// Boxes looks a box up in the agreed state: false when it is not there
// (never joined, or dropped after 30 days unheard)
type Boxes func(name string) (BoxInfo, bool)

const (
	// Grace: how long a box may be gone, by its member's doing, and still
	// count
	Grace = 30 * 24 * 3600
	// what "8 GB" of RAM shows as: MemTotal leaves out what the firmware
	// and the graphics keep
	minRAM = 7 << 30
	// 1 TB usable: the box's store's own size (dd_store_size_bytes)
	minStore = 1e12
)

// Qualify: what a box is worth to its member, from its own report as its
// heartbeat carries it (cmd/dd-control facts): a TPM 2.0 and UEFI on any
// architecture, and either general compute (4 cores, 8 GB, not a
// Raspberry Pi) or a specialist (1 TB of store, or a GPU). "" for none.
func Qualify(f map[string]float64) string {
	if f["tpm_major"] != 2 || f["uefi"] != 1 {
		return ""
	}
	switch {
	case f["cores"] >= 4 && f["ram_bytes"] >= minRAM && f["raspberry_pi"] == 0:
		return "compute"
	case f["store_size_bytes"] >= minStore:
		return "storage"
	case f["gpus"] >= 1:
		return "gpu"
	}
	return ""
}

// OurFault: the box went quiet on a release that failed on it - its last
// word was that its agent's run did not end well (agent_ok 0: refused, or
// rolled back). A box down for that is never counted as gone.
func OurFault(f map[string]float64) bool {
	v, said := f["agent_ok"]
	return said && v == 0
}

// boxesOf: the boxes whose enlistment to name stands
func (s *State) boxesOf(name string, at uint64) []string {
	var out []string
	for b, en := range s.Enlisted {
		if en.Member == name && s.owns(en, at) {
			out = append(out, b)
		}
	}
	sort.Strings(out)
	return out
}

// BoxView is one enlisted box as its member's access sees it
type BoxView struct {
	Box       string `json:"box"`
	Ticket    string `json:"ticket"`
	Since     uint64 `json:"since"`     // enlisted, ms
	Qualifies string `json:"qualifies"` // compute | storage | gpu | ""
	LastSeen  int64  `json:"lastSeen,omitempty"`
	// whether it keeps their access now, and why or why not: seen, our
	// fault, extended, gone, not qualifying
	Counts bool   `json:"counts"`
	Why    string `json:"why"`
}

// Access: "invite", "box" or "none", with their enlisted boxes and the
// admin's extension (seconds, 0 for none running)
func (s *State) Access(cfg *Config, e *Entry, at uint64, boxes Boxes) (string, []BoxView, uint64) {
	st, _, _ := s.standing(cfg, e, at)
	var grace uint64
	if x := s.Extended[e.Name]; x != nil && x.Until > at && contains(e.MemberIDs(), x.ID) {
		grace = x.Until
	}
	var out []BoxView
	counts := false
	for _, b := range s.boxesOf(e.Name, at) {
		en := s.Enlisted[b]
		v := BoxView{Box: b, Ticket: en.Ticket, Since: en.Since * 1000}
		info, known := BoxInfo{}, false
		if boxes != nil {
			info, known = boxes(b)
		}
		v.LastSeen = info.LastSeen
		v.Qualifies = Qualify(info.Facts)
		switch {
		case !known && grace > 0:
			// gone from the agreed state altogether: only the admin's word
			// keeps it
			v.Counts, v.Why = true, "extended"
		case !known:
			v.Why = "gone"
		case v.Qualifies == "":
			v.Why = "not qualifying"
		case int64(at)*1000-info.LastSeen <= Grace*1000:
			v.Counts, v.Why = true, "seen"
		case OurFault(info.Facts):
			v.Counts, v.Why = true, "our fault"
		case grace > 0:
			v.Counts, v.Why = true, "extended"
		default:
			v.Why = "gone"
		}
		counts = counts || v.Counts
		out = append(out, v)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].Box < out[j].Box })
	switch {
	case st == "left" || st == "suspended" || st == "revoked":
		return "none", out, grace
	case st == "invite":
		return "invite", out, grace
	case counts:
		return "box", out, grace
	}
	return "none", out, grace
}
