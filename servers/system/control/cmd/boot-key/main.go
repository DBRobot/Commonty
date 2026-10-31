// dd-boot-key keeps the box's join key opening across updates
// (docs/dev/attestation.md, "The join key across updates"):
//
//	dd-boot-key enrol                  the join key under an approver, at the first own-keys boot
//	dd-boot-key approve -boot a,b ...  approve booting these images (systemd-boot, then the entry's)
//	dd-boot-key approve-esp            approve what the ESP starts: the newest generations
//	dd-boot-key settle                 at boot: a new approved boot joins the good ones, the epoch moves on
//	dd-boot-key apps                   the boot applications this boot's log says ran (Authenticode, hex)
//	dd-boot-key digest FILE...         an image's Authenticode digest, as PCR 4 takes it
//
// Every command needs the TPM; a box whose join key is PCR 7's alone
// (made before this) is left as it is.
package main

import (
	"encoding/hex"
	"errors"
	"flag"
	"fmt"
	"os"
	"strings"

	"commonty/control/state/attest"
	"commonty/control/tpm"
)

type boots [][]string

func (b *boots) String() string { return fmt.Sprint(*b) }
func (b *boots) Set(s string) error {
	*b = append(*b, strings.Split(s, ","))
	return nil
}

func must(err error) {
	if err != nil {
		fmt.Fprintln(os.Stderr, "dd-boot-key:", err)
		os.Exit(1)
	}
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "dd-boot-key enrol | approve -boot a,b ... | approve-esp | settle")
		os.Exit(2)
	}
	box := tpm.Default()
	fs := flag.NewFlagSet(os.Args[1], flag.ExitOnError)
	fs.StringVar(&box.Keys, "keys", box.Keys, "the join key's files")
	fs.StringVar(&box.Dir, "dir", "/run/dd-boot-key", "working files")
	fs.StringVar(&box.Log, "log", box.Log, "the firmware's event log")
	esp := fs.String("esp", "/boot", "the ESP")
	var bs boots
	fs.Var(&bs, "boot", "one boot's applications, in order, comma-separated (repeat)")
	_ = fs.Parse(os.Args[2:])
	if t := os.Getenv("TPM2TOOLS_TCTI"); t != "" {
		box.TCTI = t
		box.NoRM = strings.HasPrefix(t, "swtpm:")
	}
	switch os.Args[1] {
	case "apps":
		raw, err := os.ReadFile(box.Log)
		must(err)
		apps, err := attest.BootApps(raw)
		must(err)
		for _, a := range apps {
			fmt.Println(hex.EncodeToString(a))
		}
		return
	case "digest":
		for _, f := range fs.Args() {
			img, err := os.ReadFile(f)
			must(err)
			d, err := attest.Authenticode(img)
			must(err)
			fmt.Println(hex.EncodeToString(d))
		}
		return
	}
	if os.Args[1] != "enrol" && !box.BootKeyed() {
		fmt.Println("dd-boot-key: the join key is PCR 7's alone; nothing to do")
		return
	}
	var err error
	switch os.Args[1] {
	case "enrol":
		err = box.Enrol()
	case "approve":
		err = box.Approve(bs)
	case "approve-esp":
		if bs, err = tpm.ESPBoots(*esp); err == nil {
			err = box.Approve(bs)
		}
	case "settle":
		err = box.Settle()
		if errors.Is(err, tpm.ErrNotApproved) {
			fmt.Println("dd-boot-key: this boot is not approved; nothing changed")
			return
		}
	default:
		err = fmt.Errorf("no such command: %s", os.Args[1])
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "dd-boot-key:", err)
		os.Exit(1)
	}
}
