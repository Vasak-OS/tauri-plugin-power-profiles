//! El gestor contra un bus de D-Bus de verdad, privado, con un
//! power-profiles-daemon de mentira. Así se prueba lo que hace con las señales
//! —un cambio de afuera, el demonio que se va— sin tocar el de la máquina ni
//! depender de que el CI lo tenga.
//!
//! Hace falta `dbus-daemon`. Si no está, las pruebas lo dicen y no fallan: no
//! son ellas las que deciden si la máquina tiene D-Bus.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use zbus::zvariant::{OwnedValue, Value};
use zbus::{connection, Connection};

use super::*;

/// Un `dbus-daemon` propio, que se cierra al terminar la prueba.
struct PrivateBus {
    child: Child,
    dir: PathBuf,
    address: String,
}

impl PrivateBus {
    fn start() -> Option<Self> {
        let dir = std::env::temp_dir().join(format!(
            "power-manager-bus-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).ok()?;
        let config = dir.join("bus.conf");
        std::fs::write(
            &config,
            r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
        )
        .ok()?;

        let mut child = match Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(e) => {
                eprintln!("sin dbus-daemon ({e}): se saltean las pruebas con bus");
                return None;
            }
        };
        let mut address = String::new();
        BufReader::new(child.stdout.take()?)
            .read_line(&mut address)
            .ok()?;
        Some(Self {
            child,
            dir,
            address: address.trim().to_string(),
        })
    }

    async fn connect(&self) -> Connection {
        connection::Builder::address(self.address.as_str())
            .unwrap()
            .build()
            .await
            .unwrap()
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn profile_maps(names: &[String]) -> Vec<HashMap<String, OwnedValue>> {
    names
        .iter()
        .map(|name| {
            HashMap::from([
                (
                    "Profile".to_string(),
                    Value::from(name.as_str()).try_to_owned().unwrap(),
                ),
                (
                    "Driver".to_string(),
                    Value::from("fake").try_to_owned().unwrap(),
                ),
            ])
        })
        .collect()
}

/// Un demonio de mentira con la interfaz de power-profiles-daemon, bajo el
/// nombre que se le pida.
macro_rules! fake_daemon {
    ($ty:ident, $interface:literal) => {
        struct $ty {
            active: String,
            profiles: Vec<String>,
            degraded: String,
        }

        #[zbus::interface(name = $interface)]
        impl $ty {
            #[zbus(property)]
            fn active_profile(&self) -> String {
                self.active.clone()
            }

            #[zbus(property)]
            fn set_active_profile(&mut self, value: String) -> zbus::fdo::Result<()> {
                if !self.profiles.contains(&value) {
                    return Err(zbus::fdo::Error::InvalidArgs(value));
                }
                self.active = value;
                Ok(())
            }

            #[zbus(property)]
            fn profiles(&self) -> Vec<HashMap<String, OwnedValue>> {
                profile_maps(&self.profiles)
            }

            #[zbus(property)]
            fn performance_degraded(&self) -> String {
                self.degraded.clone()
            }
        }

        impl $ty {
            fn new() -> Self {
                Self {
                    active: "balanced".into(),
                    profiles: vec![
                        "power-saver".into(),
                        "balanced".into(),
                        "performance".into(),
                    ],
                    degraded: String::new(),
                }
            }
        }
    };
}

fake_daemon!(NewDaemon, "org.freedesktop.UPower.PowerProfiles");
fake_daemon!(LegacyDaemon, "net.hadess.PowerProfiles");

async fn serve_new(bus: &PrivateBus) -> Connection {
    connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name(SERVICES[0].name)
        .unwrap()
        .serve_at(SERVICES[0].path, NewDaemon::new())
        .unwrap()
        .build()
        .await
        .unwrap()
}

fn recorder() -> (Notify, UnboundedReceiver<PowerState>) {
    let (tx, rx) = unbounded_channel();
    (
        Arc::new(move |state: &PowerState| {
            let _ = tx.send(state.clone());
        }),
        rx,
    )
}

async fn next(rx: &mut UnboundedReceiver<PowerState>) -> PowerState {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("el aviso no llegó")
        .expect("canal cerrado")
}

async fn manager_on(bus: &PrivateBus, notify: Notify) -> PowerManager {
    let manager = PowerManager::new(Arc::clone(&notify));
    let backend = start_on(bus.connect().await, notify).await.unwrap();
    manager.backend.set(Some(backend)).ok();
    manager
}

#[tokio::test(flavor = "multi_thread")]
async fn lee_el_demonio_con_un_solo_getall() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let _daemon = serve_new(&bus).await;
    let (notify, _rx) = recorder();
    let manager = manager_on(&bus, notify).await;

    assert_eq!(
        manager.state().await,
        PowerState {
            available: true,
            profiles: vec![
                "power-saver".into(),
                "balanced".into(),
                "performance".into()
            ],
            active_profile: Some("balanced".into()),
            performance_degraded: None,
        }
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn cambiar_el_perfil_llega_al_demonio_y_avisa_una_vez() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let daemon = serve_new(&bus).await;
    let (notify, mut rx) = recorder();
    let manager = manager_on(&bus, notify).await;

    let state = manager.set_profile("performance").await.unwrap();
    assert_eq!(state.active_profile.as_deref(), Some("performance"));
    assert_eq!(
        next(&mut rx).await.active_profile.as_deref(),
        Some("performance")
    );

    let iface = daemon
        .object_server()
        .interface::<_, NewDaemon>(SERVICES[0].path)
        .await
        .unwrap();
    assert_eq!(iface.get().await.active, "performance");

    assert!(matches!(
        manager.set_profile("turbo").await,
        Err(Error::UnknownProfile(p)) if p == "turbo"
    ));

    // La señal del demonio, si llega, no repite el aviso: el estado ya era ése.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(rx.try_recv().is_err(), "un aviso de más");
}

#[tokio::test(flavor = "multi_thread")]
async fn se_entera_de_un_cambio_hecho_desde_afuera() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let daemon = serve_new(&bus).await;
    let (notify, mut rx) = recorder();
    let manager = manager_on(&bus, notify).await;

    let iface = daemon
        .object_server()
        .interface::<_, NewDaemon>(SERVICES[0].path)
        .await
        .unwrap();
    {
        let mut fake = iface.get_mut().await;
        fake.active = "power-saver".into();
        fake.degraded = "high-operating-temperature".into();
        fake.active_profile_changed(iface.signal_context())
            .await
            .unwrap();
        fake.performance_degraded_changed(iface.signal_context())
            .await
            .unwrap();
    }

    let mut seen = next(&mut rx).await;
    if seen.performance_degraded.is_none() {
        seen = next(&mut rx).await;
    }
    assert_eq!(seen.active_profile.as_deref(), Some("power-saver"));
    assert_eq!(
        seen.performance_degraded.as_deref(),
        Some("high-operating-temperature")
    );
    assert_eq!(manager.state().await, seen, "la copia quedó al día");
}

#[tokio::test(flavor = "multi_thread")]
async fn si_el_demonio_se_va_queda_no_disponible_y_vuelve_solo() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let daemon = serve_new(&bus).await;
    let (notify, mut rx) = recorder();
    let manager = manager_on(&bus, notify).await;
    assert!(manager.state().await.available);

    drop(daemon);
    let gone = next(&mut rx).await;
    assert!(!gone.available);
    assert!(gone.profiles.is_empty());
    assert!(matches!(
        manager.set_profile("balanced").await,
        Err(Error::Unavailable)
    ));

    let _back = serve_new(&bus).await;
    let back = next(&mut rx).await;
    assert!(back.available);
    assert_eq!(back.active_profile.as_deref(), Some("balanced"));
}

#[tokio::test(flavor = "multi_thread")]
async fn con_un_demonio_viejo_usa_el_nombre_de_hadess() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let daemon = connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name(SERVICES[1].name)
        .unwrap()
        .serve_at(SERVICES[1].path, LegacyDaemon::new())
        .unwrap()
        .build()
        .await
        .unwrap();
    let (notify, mut rx) = recorder();
    let manager = manager_on(&bus, notify).await;

    assert!(manager.state().await.available);
    let state = manager.set_profile("power-saver").await.unwrap();
    assert_eq!(state.active_profile.as_deref(), Some("power-saver"));
    next(&mut rx).await;

    let iface = daemon
        .object_server()
        .interface::<_, LegacyDaemon>(SERVICES[1].path)
        .await
        .unwrap();
    assert_eq!(iface.get().await.active, "power-saver");
}

#[tokio::test(flavor = "multi_thread")]
async fn sin_ningun_demonio_arranca_no_disponible() {
    let Some(bus) = PrivateBus::start() else {
        return;
    };
    let (notify, _rx) = recorder();
    let manager = manager_on(&bus, notify).await;
    assert_eq!(manager.state().await, PowerState::default());
}
