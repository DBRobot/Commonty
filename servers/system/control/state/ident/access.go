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
	LastSeen int64              `json:"lastSeen"`        // agreed ms; 0, never heard
	Facts    map[string]float64 `json:"facts,omitempty"` // its last heartbeat's
	// the newest release the release's own boxes say they run; 0, unknown
	Fleet float64 `json:"fleet,omitempty"`
}

// AccessFacts: every fact Access reads of a box (Minimum, Qualify,
// OurFault), and no other. A box's state kept with a log item
// (state.DirItem's Boxes) keeps these alone: a judge replaying the item
// reads the box as it was then, and a heartbeat's other facts are no
// business of the log's
var AccessFacts = []string{
	"tpm_major", "uefi", "secure_boot_capable", "arch_x86_64", "arch_aarch64",
	"threads", "mem_total_bytes", "system_disk_bytes", "nics", "raspberry_pi",
	"store_size_bytes", "gpu_vram_bytes", "video_encoders", "agent_ok", "release",
}

// ForAccess: info with only the facts Access reads
func ForAccess(info BoxInfo) BoxInfo {
	out := BoxInfo{LastSeen: info.LastSeen, Fleet: info.Fleet}
	for _, k := range AccessFacts {
		if v, ok := info.Facts[k]; ok {
			if out.Facts == nil {
				out.Facts = map[string]float64{}
			}
			out.Facts[k] = v
		}
	}
	return out
}

// BoxesAt: the boxes as a log item kept them when it was ordered; nil (an
// item from before items kept them) knows none, so no box counts
func BoxesAt(m map[string]BoxInfo) Boxes {
	if m == nil {
		return nil
	}
	return func(name string) (BoxInfo, bool) {
		b, ok := m[name]
		return b, ok
	}
}

// Boxes looks a box up in the agreed state: false when it is not there
// (never joined, or dropped after 30 days unheard)
type Boxes func(name string) (BoxInfo, bool)

const (
	// Grace: how long a box may be gone, by its member's doing, and still
	// count
	Grace = 30 * 24 * 3600
	// OurFaultFor: how long a box down by our fault still counts, from its
	// last word. That word is the box's own (its release and how it went),
	// so it has an end; past it, only the admin's extension keeps it
	OurFaultFor = 90 * 24 * 3600
)

// OurFault: the box went quiet on a release that failed on it - its last
// word was that its agent's run did not end well (agent_ok 0: refused, or
// rolled back) on a release older than the one the release's own boxes
// run (fleet, 0 unknown). A box down for that is never counted as gone.
// Its word alone is not enough: with no newer release out, nothing of
// ours failed on it.
func OurFault(f map[string]float64, fleet float64) bool {
	v, said := f["agent_ok"]
	r, runs := f["release"]
	return said && v == 0 && runs && r < fleet
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
		case OurFault(info.Facts, info.Fleet) && int64(at)*1000-info.LastSeen <= OurFaultFor*1000:
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
