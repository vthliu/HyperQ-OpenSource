use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::RsaPrivateKey;
use rsa::sha2::Sha256;
use rsa::signature::Signer;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use urlencoding::encode as url_encode;

fn main() {
    let path = "/home/vth/Vth/hyperq-rs/keys/private_key.pem";
    let private_key = RsaPrivateKey::read_pkcs8_pem_file(path).unwrap();
    let signing_key = SigningKey::<Sha256>::new(private_key);
    
    let payload = "timestamp=1785846414693";
    let signature = signing_key.sign(payload.as_bytes());
    // In rsa 0.9, signature returns an object, we need to convert to bytes?
    // Let's print type of signature to see if it has to_bytes() or similar.
    let sig_bytes = signature.to_bytes();
    let b64 = BASE64_STANDARD.encode(sig_bytes);
    let final_sig = url_encode(&b64);
    println!("Signature: {}", final_sig);
}
