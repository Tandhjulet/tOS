use ovmf_prebuilt::{Arch, FileType, Prebuilt, Source};
use std::env;
use std::process::{Command, exit};

fn main() {
    // read env variables that were set in build script
    let uefi_path = env!("UEFI_PATH");
    let bios_path = env!("BIOS_PATH");

    // parse mode from CLI
    let args: Vec<String> = env::args().collect();
    let prog = &args[0];

    // choose whether to start the UEFI or BIOS image
    let uefi = match args.get(1).map(|s| s.to_lowercase()) {
        Some(ref s) if s == "uefi" => true,
        Some(ref s) if s == "bios" => false,
        Some(ref s) if s == "-h" || s == "--help" => {
            println!("Usage: {prog} [uefi|bios]");
            println!("  uefi  - boot using OVMF (UEFI)");
            println!("  bios  - boot using legacy BIOS");
            exit(0);
        }
        _ => {
            eprintln!("Usage: {prog} [uefi|bios]");
            exit(1);
        }
    };

    let mut cmd = Command::new("qemu-system-x86_64");
    // enable the guest to exit qemu
    cmd.arg("-device")
        .arg("isa-debug-exit,iobase=0xf4,iosize=0x04");

    // configure network
    cmd.arg("-netdev").arg("user,id=net0");
    cmd.arg("-object")
        .arg("filter-dump,id=f1,netdev=net0,file=/tmp/dhcp.pcap");
    cmd.arg("-device").arg("e1000,netdev=net0");

    // MCFG, PCIe, etc.
    cmd.arg("-M").arg("q35");

    // drives
    cmd.arg("-drive")
        .arg("file=../nvm.img,if=none,id=nvm,format=raw");
    cmd.arg("-device").arg("nvme,serial=deadbeef,drive=nvm");

    // don't store
    cmd.arg("-snapshot");

    cmd.arg("-monitor").arg("stdio");

    if uefi {
        let prebuilt =
            Prebuilt::fetch(Source::LATEST, "target/ovmf").expect("failed to update prebuilt");

        let code = prebuilt.get_file(Arch::X64, FileType::Code);
        let vars = prebuilt.get_file(Arch::X64, FileType::Vars);

        cmd.arg("-drive")
            .arg(format!("format=raw,file={uefi_path}"));
        cmd.arg("-drive").arg(format!(
            "if=pflash,format=raw,unit=0,file={},readonly=on",
            code.display()
        ));
        // copy vars and enable rw instead of snapshot if you want to store data (e.g. enroll secure boot keys)
        cmd.arg("-drive").arg(format!(
            "if=pflash,format=raw,unit=1,file={},snapshot=on",
            vars.display()
        ));
    } else {
        cmd.arg("-drive")
            .arg(format!("format=raw,file={bios_path}"));
    }

    let mut child = cmd.spawn().expect("failed to start qemu-system-x86_64");
    let status = child.wait().expect("failed to wait on qemu");
    match status.code().unwrap_or(1) {
        0x10 => 0, // success
        0x11 => 1, // failure
        _ => 2,    // unknown fault
    };
}
