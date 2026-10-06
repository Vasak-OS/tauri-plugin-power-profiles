use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use zbus::zvariant::{OwnedValue, Value};

/// Nombre del evento que el plugin emite cada vez que cambia algo del perfil:
/// el activo, la lista, la degradación, o que el demonio aparezca o se vaya.
pub const POWER_STATE_EVENT: &str = "power-profile-changed";

/// Todo lo que el frontend necesita para dibujar el selector, en una sola
/// lectura y sin ir al bus: es la copia que el plugin mantiene al día con las
/// señales de power-profiles-daemon.
///
/// Todo lo que sale hacia el frontend va en camelCase, igual que en el resto de
/// los complementos del taller y que en `guest-js/index.ts`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PowerState {
    /// Falso cuando power-profiles-daemon no está instalado o no responde. El
    /// selector se muestra «no disponible», no roto.
    pub available: bool,
    /// Los perfiles que ofrece este equipo, en el orden del demonio
    /// (`power-saver`, `balanced` y, si el hardware lo soporta, `performance`).
    pub profiles: Vec<String>,
    pub active_profile: Option<String>,
    /// Por qué el perfil de rendimiento está limitado (`lap-detected`,
    /// `high-operating-temperature`), o nada si no lo está.
    pub performance_degraded: Option<String>,
}

impl PowerState {
    /// El estado a partir de un `GetAll` de la interfaz.
    pub fn from_properties(props: &HashMap<String, OwnedValue>) -> Self {
        let mut state = PowerState {
            available: true,
            ..PowerState::default()
        };
        state.apply_changes(props.iter().map(|(k, v)| (k.as_str(), &**v)));
        state
    }

    /// Aplica las propiedades que trae un `PropertiesChanged`. Devuelve si algo
    /// cambió, para no emitir un evento que no dice nada nuevo.
    pub fn apply_changes<'a, I>(&mut self, changed: I) -> bool
    where
        I: IntoIterator<Item = (&'a str, &'a Value<'a>)>,
    {
        let before = self.clone();

        for (name, value) in changed {
            match name {
                "ActiveProfile" => self.active_profile = string_of(value),
                "Profiles" => self.profiles = profile_names(value),
                "PerformanceDegraded" => {
                    self.performance_degraded = string_of(value).filter(|s| !s.is_empty())
                }
                _ => {}
            }
        }

        *self != before
    }
}

/// Quita las capas de variante que a veces envuelven el valor.
fn unwrap_variant<'a>(value: &'a Value<'a>) -> &'a Value<'a> {
    match value {
        Value::Value(inner) => unwrap_variant(inner),
        other => other,
    }
}

fn string_of(value: &Value<'_>) -> Option<String> {
    match unwrap_variant(value) {
        Value::Str(s) => Some(s.to_string()),
        _ => None,
    }
}

/// Los nombres de perfil de la propiedad `Profiles`, que es `aa{sv}`: una lista
/// de diccionarios con la clave `Profile` y otras (`Driver`, `CpuDriver`…) que
/// acá no hacen falta.
pub fn profile_names(value: &Value<'_>) -> Vec<String> {
    let Value::Array(items) = unwrap_variant(value) else {
        return Vec::new();
    };

    items
        .iter()
        .filter_map(|item| match unwrap_variant(item) {
            Value::Dict(dict) => dict.iter().find_map(|(key, value)| {
                matches!(unwrap_variant(key), Value::Str(k) if k.as_str() == "Profile")
                    .then(|| string_of(value))
                    .flatten()
            }),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profiles_value(names: &[&str]) -> Value<'static> {
        let list: Vec<HashMap<&str, Value>> = names
            .iter()
            .map(|name| {
                HashMap::from([
                    ("Profile", Value::from(name.to_string())),
                    ("Driver", Value::from("platform_profile")),
                ])
            })
            .collect();
        Value::from(list).try_to_owned().unwrap().into()
    }

    #[test]
    fn lee_los_nombres_de_la_lista_de_perfiles() {
        let value = profiles_value(&["power-saver", "balanced", "performance"]);
        assert_eq!(
            profile_names(&value),
            vec!["power-saver", "balanced", "performance"]
        );
    }

    #[test]
    fn la_lista_puede_venir_envuelta_en_una_variante() {
        let value = Value::Value(Box::new(profiles_value(&["balanced"])));
        assert_eq!(profile_names(&value), vec!["balanced"]);
    }

    #[test]
    fn un_valor_que_no_es_lista_no_inventa_perfiles() {
        assert!(profile_names(&Value::from("balanced")).is_empty());
        assert!(profile_names(&Value::from(3u32)).is_empty());
    }

    #[test]
    fn un_diccionario_sin_la_clave_profile_se_saltea() {
        let list: Vec<HashMap<&str, Value>> = vec![
            HashMap::from([("Driver", Value::from("x"))]),
            HashMap::from([("Profile", Value::from("balanced"))]),
        ];
        assert_eq!(profile_names(&Value::from(list)), vec!["balanced"]);
    }

    #[test]
    fn aplica_los_cambios_y_dice_si_hubo_alguno() {
        let mut state = PowerState {
            available: true,
            profiles: vec!["power-saver".into(), "balanced".into()],
            active_profile: Some("balanced".into()),
            performance_degraded: None,
        };

        let active = Value::from("power-saver");
        assert!(state.apply_changes([("ActiveProfile", &active)]));
        assert_eq!(state.active_profile.as_deref(), Some("power-saver"));

        assert!(
            !state.apply_changes([("ActiveProfile", &active)]),
            "el mismo valor no es un cambio"
        );

        let unknown = Value::from(true);
        assert!(
            !state.apply_changes([("PerformanceInhibited", &unknown)]),
            "una propiedad que no se usa no es un cambio"
        );
    }

    #[test]
    fn la_degradacion_vacia_es_ninguna() {
        let mut state = PowerState::default();
        let reason = Value::from("lap-detected");
        state.apply_changes([("PerformanceDegraded", &reason)]);
        assert_eq!(state.performance_degraded.as_deref(), Some("lap-detected"));

        let empty = Value::from("");
        state.apply_changes([("PerformanceDegraded", &empty)]);
        assert_eq!(state.performance_degraded, None);
    }

    #[test]
    fn arma_el_estado_desde_un_getall() {
        let props: HashMap<String, OwnedValue> = HashMap::from([
            (
                "ActiveProfile".to_string(),
                OwnedValue::try_from(Value::from("performance")).unwrap(),
            ),
            (
                "Profiles".to_string(),
                OwnedValue::try_from(profiles_value(&["balanced", "performance"])).unwrap(),
            ),
            (
                "PerformanceDegraded".to_string(),
                OwnedValue::try_from(Value::from("")).unwrap(),
            ),
        ]);

        assert_eq!(
            PowerState::from_properties(&props),
            PowerState {
                available: true,
                profiles: vec!["balanced".into(), "performance".into()],
                active_profile: Some("performance".into()),
                performance_degraded: None,
            }
        );
    }

    /// Las claves tienen que ser las que declara `guest-js/index.ts`.
    #[test]
    fn las_claves_salen_en_camel_case() {
        let json = serde_json::to_value(PowerState::default()).unwrap();
        let mut keys: Vec<_> = json.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "activeProfile",
                "available",
                "performanceDegraded",
                "profiles"
            ]
        );
    }
}
