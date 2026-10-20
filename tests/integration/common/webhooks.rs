use linewright::webhook::{Address, Listener, Secret, Webhooks};

pub const SECRET: &str = "It's a Secret to Everybody";

pub fn webhooks(address: &str) -> Webhooks {
    Webhooks {
        address: address.parse::<Address>().unwrap(),
        secret: Secret::new(SECRET.into()).unwrap(),
    }
}

pub fn signature(body: &str) -> String {
    use hmac::Mac;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(SECRET.as_bytes()).unwrap();
    mac.update(body.as_bytes());
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// A delivery as it reaches the listener, with each header GitHub sends it
/// with left out when `None`.
pub struct Request<'a> {
    pub event: Option<&'a str>,
    pub content_type: Option<&'a str>,
    pub signature: Option<String>,
    pub body: String,
}

pub struct Response {
    pub status: reqwest::StatusCode,
    pub body: String,
}

impl<'a> Request<'a> {
    pub fn signed_json(body: &str) -> Request<'a> {
        Request {
            event: None,
            content_type: Some("application/json"),
            signature: Some(signature(body)),
            body: body.to_string(),
        }
    }

    pub fn signed_form(payload: &str) -> Request<'a> {
        let body = form_urlencoded::Serializer::new(String::new())
            .append_pair("payload", payload)
            .finish();
        Request {
            event: None,
            content_type: Some("application/x-www-form-urlencoded"),
            signature: Some(signature(&body)),
            body,
        }
    }

    pub async fn send(&self, listener: &Listener) -> Response {
        let mut request = reqwest::Client::new()
            .post(format!("http://{}/", listener.local_address()))
            .header("X-GitHub-Delivery", "72d3162e-cc78-11e3-81ab-4c9367dc0958")
            .body(self.body.clone());
        if let Some(signature) = &self.signature {
            request = request.header("X-Hub-Signature-256", signature);
        }
        if let Some(event) = self.event {
            request = request.header("X-GitHub-Event", event);
        }
        if let Some(content_type) = self.content_type {
            request = request.header("Content-Type", content_type);
        }
        let response = request.send().await.unwrap();
        Response {
            status: response.status(),
            body: response.text().await.unwrap(),
        }
    }
}

pub async fn signed(listener: &Listener, event: &str, body: &str) -> reqwest::StatusCode {
    let request = Request {
        event: Some(event),
        ..Request::signed_json(body)
    };
    request.send(listener).await.status
}

pub async fn signed_form(listener: &Listener, event: &str, payload: &str) -> reqwest::StatusCode {
    let request = Request {
        event: Some(event),
        ..Request::signed_form(payload)
    };
    request.send(listener).await.status
}
