//! La copia del estado de power-profiles-daemon y el oyente que la mantiene.
//!
//! El plugin no le pregunta nada al demonio cuando el frontend pide el estado:
//! lo lee una vez con `GetAll` y después lo actualiza con las señales
//! `PropertiesChanged` y `NameOwnerChanged`. Abrir el selector cuesta un
//! `Mutex` y un `clone`, no un viaje por el bus; y nada sondea.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use futures_util::stream::{select_all, StreamExt};
use tokio::sync::OnceCell;
use zbus::{
    message::Type as MessageType,
    zvariant::{OwnedValue, Value},
    Connection, MatchRule, MessageStream,
};

use crate::models::PowerState;
use crate::{Error, Result};

const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";

/// Un nombre por el que power-profiles-daemon se publica en el bus.
#[derive(Debug, PartialEq, Eq)]
pub struct Service {
    pub name: &'static str,
    pub path: &'static str,
    pub interface: &'static str,
}

/// Los nombres en orden de preferencia. Desde la 0.20 el demonio se publica
/// como `org.freedesktop.UPower.PowerProfiles` y mantiene `net.hadess` por
/// compatibilidad; las versiones anteriores sólo tienen el viejo. Se prueba el
/// nuevo primero para que el plugin siga andando el día que el viejo se retire.
pub const SERVICES: [Service; 2] = [
    Service {
        name: "org.freedesktop.UPower.PowerProfiles",
        path: "/org/freedesktop/UPower/PowerProfiles",
        interface: "org.freedesktop.UPower.PowerProfiles",
    },
    Service {
        name: "net.hadess.PowerProfiles",
        path: "/net/hadess/PowerProfiles",
        interface: "net.hadess.PowerProfiles",
    },
];

/// Quién se entera de que el estado cambió. En el plugin es un `emit` de
/// Tauri; en las pruebas, cualquier cosa.
pub type Notify = Arc<dyn Fn(&PowerState) + Send + Sync>;

struct Backend {
    conn: Connection,
    /// Cuál de los `SERVICES` contestó la última vez, o ninguno.
    service: Mutex<Option<&'static Service>>,
    state: Mutex<PowerState>,
}

/// El estado del plugin que guarda Tauri.
///
/// La conexión se abre la primera vez que alguien la necesita —o al arrancar,
/// en segundo plano— y nunca en el `setup` del plugin: una aplicación no tiene
/// por qué esperar al bus del sistema para abrir su ventana.
pub struct PowerManager {
    backend: OnceCell<Option<Arc<Backend>>>,
    notify: Notify,
}

impl PowerManager {
    pub fn new(notify: Notify) -> Self {
        Self {
            backend: OnceCell::new(),
            notify,
        }
    }

    async fn backend(&self) -> Option<Arc<Backend>> {
        self.backend
            .get_or_init(|| async {
                match start(Arc::clone(&self.notify)).await {
                    Ok(backend) => Some(backend),
                    Err(e) => {
                        log::warn!("power-manager: sin bus del sistema: {e}");
                        None
                    }
                }
            })
            .await
            .clone()
    }

    /// Abre la conexión y arranca el oyente si todavía no estaba.
    pub async fn warm_up(&self) {
        let _ = self.backend().await;
    }

    /// El estado como lo tiene el plugin, sin ir al bus.
    pub async fn state(&self) -> PowerState {
        match self.backend().await {
            Some(backend) => backend.state.lock().map(|s| s.clone()).unwrap_or_default(),
            None => PowerState::default(),
        }
    }

    /// Cambia el perfil activo y devuelve el estado nuevo.
    pub async fn set_profile(&self, profile: &str) -> Result<PowerState> {
        let backend = self.backend().await.ok_or(Error::Unavailable)?;

        let (service, known) = {
            let service = *backend.service.lock().map_err(|_| Error::Unavailable)?;
            let state = backend.state.lock().map_err(|_| Error::Unavailable)?;
            (service, state.profiles.clone())
        };
        let service = service.ok_or(Error::Unavailable)?;
        check_profile(profile, &known)?;

        backend
            .conn
            .call_method(
                Some(service.name),
                service.path,
                Some(PROPERTIES_INTERFACE),
                "Set",
                &(service.interface, "ActiveProfile", Value::from(profile)),
            )
            .await?;

        // La señal va a llegar igual, pero quien llama puede preguntar el
        // estado en la línea siguiente y tiene que ver el perfil que acaba de
        // poner. Si la señal llega después, no cambia nada y no emite de nuevo.
        let active = Value::from(profile);
        let snapshot = {
            let mut state = backend.state.lock().map_err(|_| Error::Unavailable)?;
            let changed = state.apply_changes([("ActiveProfile", &active)]);
            (changed, state.clone())
        };
        if snapshot.0 {
            (self.notify)(&snapshot.1);
        }
        Ok(snapshot.1)
    }
}

/// Que el perfil pedido sea uno de los que el equipo ofrece. El demonio
/// también lo rechazaría, pero con un error de D-Bus que no dice cuál.
pub fn check_profile(profile: &str, known: &[String]) -> Result<()> {
    if known.iter().any(|p| p == profile) {
        Ok(())
    } else {
        Err(Error::UnknownProfile(profile.to_string()))
    }
}

/// `GetAll` contra un nombre del demonio.
async fn read_service(conn: &Connection, service: &Service) -> Result<PowerState> {
    let reply = conn
        .call_method(
            Some(service.name),
            service.path,
            Some(PROPERTIES_INTERFACE),
            "GetAll",
            &(service.interface,),
        )
        .await?;
    let props: HashMap<String, OwnedValue> = reply.body().deserialize()?;
    Ok(PowerState::from_properties(&props))
}

/// Vuelve a leer todo desde cero, probando los nombres en orden. Es lo que se
/// hace al arrancar y cada vez que el demonio aparece o se va.
async fn reload(backend: &Backend) -> PowerState {
    let mut found = None;
    for service in &SERVICES {
        match read_service(&backend.conn, service).await {
            Ok(state) => {
                found = Some((service, state));
                break;
            }
            Err(e) => log::debug!("power-manager: {} no contesta: {e}", service.name),
        }
    }

    let (service, state) = match found {
        Some((service, state)) => (Some(service), state),
        None => (None, PowerState::default()),
    };

    if let Ok(mut current) = backend.service.lock() {
        *current = service;
    }
    if let Ok(mut current) = backend.state.lock() {
        *current = state.clone();
    }
    state
}

/// Las suscripciones van **antes** del primer `GetAll`: al revés, un cambio
/// que llegara entre la lectura y la suscripción se perdería sin aviso.
async fn subscribe(conn: &Connection) -> Result<Vec<MessageStream>> {
    let mut streams = Vec::new();

    for service in &SERVICES {
        let owner = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .sender("org.freedesktop.DBus")?
            .interface("org.freedesktop.DBus")?
            .member("NameOwnerChanged")?
            .arg(0, service.name)?
            .build();
        streams.push(MessageStream::for_match_rule(owner, conn, None).await?);

        let props = MatchRule::builder()
            .msg_type(MessageType::Signal)
            .interface(PROPERTIES_INTERFACE)?
            .member("PropertiesChanged")?
            .path(service.path)?
            .arg(0, service.interface)?
            .build();
        streams.push(MessageStream::for_match_rule(props, conn, None).await?);
    }

    Ok(streams)
}

async fn start(notify: Notify) -> Result<Arc<Backend>> {
    start_on(Connection::system().await?, notify).await
}

/// Arranca sobre una conexión dada: el bus del sistema en el plugin, uno
/// privado en las pruebas.
async fn start_on(conn: Connection, notify: Notify) -> Result<Arc<Backend>> {
    let streams = subscribe(&conn).await?;

    let backend = Arc::new(Backend {
        conn,
        service: Mutex::new(None),
        state: Mutex::new(PowerState::default()),
    });
    reload(&backend).await;

    tauri::async_runtime::spawn(listen(Arc::clone(&backend), streams, notify));
    Ok(backend)
}

/// Qué hacer con una señal.
#[derive(Debug, PartialEq, Eq)]
enum Signal {
    /// El demonio apareció o se fue: se relee todo.
    OwnerChanged,
    /// Cambiaron propiedades del objeto en esta ruta.
    Properties(String),
    Other,
}

fn classify(member: Option<&str>, path: Option<&str>) -> Signal {
    match (member, path) {
        (Some("NameOwnerChanged"), _) => Signal::OwnerChanged,
        (Some("PropertiesChanged"), Some(path)) => Signal::Properties(path.to_string()),
        _ => Signal::Other,
    }
}

async fn listen(backend: Arc<Backend>, streams: Vec<MessageStream>, notify: Notify) {
    let mut signals = select_all(streams);

    while let Some(message) = signals.next().await {
        let Ok(message) = message else { continue };
        let header = message.header();
        let signal = classify(
            header.member().map(|m| m.as_str()),
            header.path().map(|p| p.as_str()),
        );

        let changed = match signal {
            Signal::OwnerChanged => {
                let before = backend.state.lock().map(|s| s.clone()).unwrap_or_default();
                let after = reload(&backend).await;
                (before != after).then_some(after)
            }
            Signal::Properties(path) => {
                // Si el demonio publica los dos nombres, el mismo cambio llega
                // dos veces; sólo cuenta el del nombre que se está usando.
                let current = backend.service.lock().ok().and_then(|s| *s);
                if current.map(|s| s.path) != Some(path.as_str()) {
                    continue;
                }
                let body = message.body();
                let Ok((_, changed, _)) =
                    body.deserialize::<(String, HashMap<String, Value<'_>>, Vec<String>)>()
                else {
                    continue;
                };
                let Ok(mut state) = backend.state.lock() else {
                    continue;
                };
                state
                    .apply_changes(changed.iter().map(|(k, v)| (k.as_str(), v)))
                    .then(|| state.clone())
            }
            Signal::Other => None,
        };

        if let Some(state) = changed {
            notify(&state);
        }
    }

    log::warn!("power-manager: se cerró la conexión con el bus del sistema");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefiere_el_nombre_nuevo_del_demonio() {
        assert_eq!(SERVICES[0].name, "org.freedesktop.UPower.PowerProfiles");
        assert_eq!(SERVICES[1].name, "net.hadess.PowerProfiles");
        for service in &SERVICES {
            assert_eq!(service.name, service.interface);
            assert_eq!(
                service.path,
                format!("/{}", service.name.replace('.', "/")),
                "la ruta del objeto es el nombre con barras"
            );
        }
    }

    #[test]
    fn rechaza_un_perfil_que_el_equipo_no_ofrece() {
        let known = vec!["power-saver".to_string(), "balanced".to_string()];
        assert!(check_profile("balanced", &known).is_ok());
        assert!(matches!(
            check_profile("performance", &known),
            Err(Error::UnknownProfile(p)) if p == "performance"
        ));
        assert!(
            check_profile("balanced", &[]).is_err(),
            "sin demonio no hay perfiles"
        );
    }

    #[test]
    fn clasifica_las_senales() {
        assert_eq!(
            classify(Some("NameOwnerChanged"), Some("/org/freedesktop/DBus")),
            Signal::OwnerChanged
        );
        assert_eq!(
            classify(Some("PropertiesChanged"), Some("/net/hadess/PowerProfiles")),
            Signal::Properties("/net/hadess/PowerProfiles".into())
        );
        assert_eq!(classify(Some("PropertiesChanged"), None), Signal::Other);
        assert_eq!(classify(Some("Foo"), Some("/x")), Signal::Other);
    }

    #[tokio::test]
    async fn sin_bus_el_estado_es_no_disponible_y_no_un_error() {
        // Un gestor cuyo arranque falló se comporta como «no hay demonio».
        let manager = PowerManager::new(Arc::new(|_| {}));
        manager.backend.set(None).ok();

        assert_eq!(manager.state().await, PowerState::default());
        assert!(!manager.state().await.available);
        assert!(matches!(
            manager.set_profile("balanced").await,
            Err(Error::Unavailable)
        ));
    }
}

#[cfg(test)]
mod bus_tests;
