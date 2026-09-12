use nkscan::{
    device,
    protocol::cdbs::{Abort, TestUnitReady},
    transport::Data,
};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let scanners = device::list();
    let scanner = scanners.first().ok_or("No scanner")?;
    // Session::open stages the film. Recovery must send STOP before that preamble.
    let mut transport = scanner.open()?;
    if !std::env::args().any(|a| a == "--ready-only") {
        println!("Sending abort directly to {scanner}");
        let stopped =
            transport.execute(&Abort.cdb(), Data::None, std::time::Duration::from_secs(10))?;
        println!("Abort: {:?} {:?}", stopped.status, stopped.sense);
    }
    for i in 1..=3 {
        let ready = transport.execute(
            &TestUnitReady.cdb(),
            Data::None,
            std::time::Duration::from_secs(10),
        )?;
        println!("Ready ({i}): {:?} {:?}", ready.status, ready.sense);
        if ready.status == nkscan::transport::Status::Good {
            break;
        }
    }
    Ok(())
}
