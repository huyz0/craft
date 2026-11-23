use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
};
use ring::signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use crate::{Result, reject};

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Envelope {
    payload_type: String,
    payload: String,
    signatures: Vec<Signature>,
}

#[derive(Deserialize, JsonSchema)]
struct Signature {
    sig: String,
}

pub fn public_key(encoded: &str) -> Result<[u8; 32]> {
    decode(encoded)?
        .try_into()
        .map_err(|_| "the approved Ed25519 public key must be 32 bytes".into())
}

pub fn seal(
    payload: &[u8],
    payload_type: &str,
    private_key: &[u8],
    public_key: &[u8; 32],
    key_purpose: &str,
) -> Result<Vec<u8>> {
    let key = Ed25519KeyPair::from_pkcs8_maybe_unchecked(private_key)
        .map_err(|_| "invalid Ed25519 PKCS#8 signing key")?;
    if key.public_key().as_ref() != public_key {
        return reject(format!(
            "private key does not match the approved {key_purpose} signing key"
        ));
    }
    let signature = key.sign(&preauthentication_encoding(payload_type, payload));
    Ok(serde_json::to_vec(&json!({
        "payloadType": payload_type, "payload": STANDARD.encode(payload),
        "signatures": [{"sig": STANDARD.encode(signature.as_ref())}],
    }))?)
}

pub fn verify(archive: &[u8], payload_type: &str, public_key: &[u8; 32]) -> Result<Vec<u8>> {
    verify_envelope(&serde_json::from_slice(archive)?, payload_type, public_key)
}

pub fn verify_envelope(
    envelope: &Envelope,
    payload_type: &str,
    public_key: &[u8; 32],
) -> Result<Vec<u8>> {
    if envelope.payload_type != payload_type || !(1..=8).contains(&envelope.signatures.len()) {
        return reject(
            "expected a DSSE envelope of the approved payload type with one to eight signatures",
        );
    }
    let payload = decode(&envelope.payload)?;
    let signatures: Vec<_> = envelope
        .signatures
        .iter()
        .map(|signature| decode(&signature.sig))
        .collect::<Result<_>>()?;
    let pae = preauthentication_encoding(payload_type, &payload);
    let verifier = UnparsedPublicKey::new(&ED25519, public_key);
    if !signatures
        .iter()
        .any(|signature| verifier.verify(&pae, signature).is_ok())
    {
        return reject("no DSSE signature matches the approved Ed25519 public key");
    }
    Ok(payload)
}

fn preauthentication_encoding(payload_type: &str, payload: &[u8]) -> Vec<u8> {
    let mut pae = format!(
        "DSSEv1 {} {} {} ",
        payload_type.len(),
        payload_type,
        payload.len()
    )
    .into_bytes();
    pae.extend_from_slice(payload);
    pae
}

fn decode(text: &str) -> Result<Vec<u8>> {
    [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD]
        .into_iter()
        .find_map(|encoding| encoding.decode(text).ok())
        .ok_or_else(|| "invalid base64 in DSSE evidence".into())
}
