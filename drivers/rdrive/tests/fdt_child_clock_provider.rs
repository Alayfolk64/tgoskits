use core::ptr::NonNull;
use std::vec::Vec;

use fdt_edit::{Fdt, Node, Property};
use rdrive::{
    DriverGeneric, Platform,
    probe::{OnProbeError, fdt::ProbeFdt},
    probe_all,
    register::{DriverRegister, ProbeKind, ProbeLevel, ProbePriority},
};

struct ScmiTransport;

impl DriverGeneric for ScmiTransport {
    fn name(&self) -> &str {
        "test-scmi-transport"
    }
}

impl rdif_clk::Interface for ScmiTransport {
    fn perper_enable(&mut self) {}

    fn enable(&mut self, _id: rdif_clk::ClockId) -> Result<(), rdrive::KError> {
        Ok(())
    }

    fn get_rate(&self, _id: rdif_clk::ClockId) -> Result<u64, rdrive::KError> {
        Ok(50_000_000)
    }

    fn set_rate(&mut self, _id: rdif_clk::ClockId, _rate: u64) -> Result<(), rdrive::KError> {
        Ok(())
    }
}

struct ClockConsumer;

impl DriverGeneric for ClockConsumer {
    fn name(&self) -> &str {
        "clock-consumer"
    }
}

fn probe_scmi_transport(probe: ProbeFdt<'_>) -> Result<(), OnProbeError> {
    let (info, platform) = probe.into_parts();
    let clock_phandle = rdrive::probe::fdt::child_nodes(info.node)
        .into_iter()
        .find(|child| {
            child
                .as_node()
                .get_property("reg")
                .and_then(|property| property.get_u32())
                == Some(0x14)
        })
        .and_then(|child| child.as_node().phandle())
        .ok_or_else(|| OnProbeError::other("SCMI clock protocol phandle was not found"))?;
    platform.register(ScmiTransport);
    platform
        .register_fdt_phandle(clock_phandle, rdif_clk::Clk::new(ScmiTransport))
        .map_err(|error| OnProbeError::other(error.to_string()))?;
    Ok(())
}

fn probe_clock_consumer(probe: ProbeFdt<'_>) -> Result<(), OnProbeError> {
    let clock = probe
        .info()
        .find_clock_line_by_name("ciu")?
        .ok_or_else(|| OnProbeError::other("ciu clock was not resolved"))?;
    clock.enable()?;
    clock.set_rate(50_000_000)?;
    assert_eq!(clock.rate()?, 50_000_000);
    probe.into_platform_device().register(ClockConsumer);
    Ok(())
}

static SCMI_TRANSPORT_REGISTER: DriverRegister = DriverRegister {
    name: "test SCMI transport",
    level: ProbeLevel::PostKernel,
    priority: ProbePriority::CLK,
    probe_kinds: &[ProbeKind::Fdt {
        compatibles: &["test,scmi-transport"],
        on_probe: probe_scmi_transport,
    }],
};

static CLOCK_CONSUMER_REGISTER: DriverRegister = DriverRegister {
    name: "test child clock consumer",
    level: ProbeLevel::PostKernel,
    priority: ProbePriority::DEFAULT,
    probe_kinds: &[ProbeKind::Fdt {
        compatibles: &["test,clock-consumer"],
        on_probe: probe_clock_consumer,
    }],
};

#[test]
fn child_clock_protocol_exposes_the_transport_clock_capability() {
    let mut fdt = Fdt::new();
    let root = fdt.root_id();
    let transport = fdt.add_node(
        root,
        node_with_props(
            "scmi",
            &[
                prop_strs("compatible", &["test,scmi-transport"]),
                prop_u32s("phandle", &[1]),
                prop_u32s("#address-cells", &[1]),
                prop_u32s("#size-cells", &[0]),
            ],
        ),
    );
    fdt.add_node(
        transport,
        node_with_props(
            "protocol@14",
            &[
                prop_u32s("reg", &[0x14]),
                prop_u32s("phandle", &[2]),
                prop_u32s("#clock-cells", &[1]),
            ],
        ),
    );
    fdt.add_node(
        root,
        node_with_props(
            "mmc@fe2c0000",
            &[
                prop_strs("compatible", &["test,clock-consumer"]),
                prop_u32s("clocks", &[2, 3]),
                prop_strs("clock-names", &["ciu"]),
            ],
        ),
    );

    let encoded = fdt.encode();
    let dtb = Box::leak(encoded.as_ref().to_vec().into_boxed_slice());
    rdrive::init(Platform::Fdt {
        addr: NonNull::new(dtb.as_mut_ptr()).expect("encoded FDT address is non-null"),
    })
    .expect("FDT platform should initialize");
    rdrive::register_add(SCMI_TRANSPORT_REGISTER.clone());
    rdrive::register_add(CLOCK_CONSUMER_REGISTER.clone());

    probe_all(true).expect("SCMI child clock provider should satisfy the consumer");
    assert!(rdrive::get_one::<ClockConsumer>().is_some());
}

fn node_with_props(name: &str, props: &[Property]) -> Node {
    let mut node = Node::new(name);
    for prop in props {
        node.set_property(prop.clone());
    }
    node
}

fn prop_u32s(name: &str, values: &[u32]) -> Property {
    let mut data = Vec::new();
    for value in values {
        data.extend_from_slice(&value.to_be_bytes());
    }
    Property::new(name, data)
}

fn prop_strs(name: &str, values: &[&str]) -> Property {
    let mut data = Vec::new();
    for value in values {
        data.extend_from_slice(value.as_bytes());
        data.push(0);
    }
    Property::new(name, data)
}
