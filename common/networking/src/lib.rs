use cookie_store::CookieStore;
use ureq::Cookie;
use cookie::SameSite;
use ureq::{json, serde_json, AgentBuilder};
use std::error::Error;
use url::Url;

use time::OffsetDateTime;

pub type Agent = ureq::Agent;

#[allow(non_snake_case)]
#[derive(Debug, serde::Deserialize, Clone)]
pub struct FlareSolverrResponse {
    pub status: String,
    pub message: String,
    pub solution: FlareSolverrSolution,
    pub startTimestamp: u64,
    pub endTimestamp: u64,
    pub version: String,
}

#[allow(non_snake_case)]
#[derive(Debug, serde::Deserialize, Clone)]
pub struct FlareSolverrSolution {
    pub url: String,
    pub status: u16,
    pub cookies: Vec<FlareSolverrCookie>,
    pub userAgent: String,
    pub headers: serde_json::Value,
    pub response: String,
}

#[allow(non_snake_case)]
#[derive(Debug, serde::Deserialize, Clone)]
pub struct FlareSolverrCookie {
    pub domain: String,
    pub expiry: Option<u64>,
    pub httpOnly: bool,
    pub name: String,
    pub path: String,
    pub sameSite: String,
    pub secure: bool,
    pub value: String,
}

pub fn build_ureq_agent(user_agent: Option<&str>, store: Option<CookieStore>) -> Agent {
    let builder = AgentBuilder::new()
        .redirects(5)
        .user_agent(user_agent.unwrap_or_default())
        .cookie_store(store.unwrap_or_default());

        
    builder.build()
}

fn convert_flaresolverr_cookies_to_ureq_cookies(mut store: CookieStore, cookies: Vec<FlareSolverrCookie>) -> CookieStore {
    for cookie in cookies {        
        let same_site = match cookie.sameSite.as_str() {
            "Strict" => SameSite::Strict,
            "Lax" => SameSite::Lax,
            "None" => SameSite::None,
            _ => SameSite::None,
        };

        let mut cookie_builder = Cookie::build(cookie.name, cookie.value)
            .domain(&cookie.domain)
            .path(&cookie.path)
            .http_only(cookie.httpOnly)
            .secure(cookie.secure)
            .path(&cookie.path)
            .same_site(same_site);

        if let Some(expiry) = cookie.expiry {
            cookie_builder = cookie_builder.expires(OffsetDateTime::from_unix_timestamp(expiry as i64));
        }

        
        let request_url = Url::parse(format!("https://{}", &cookie.domain).as_str()).unwrap();

        let result = store.insert_raw(&cookie_builder.finish(), &request_url);

        if let Err(e) = result {
            eprintln!("Error inserting cookie: {}", e);
        }
    }

    store
}



pub fn build_flaresolverr_client(url: &str, flaresolverr_url: &str) -> Result<Agent, Box<dyn Error>> {
    let payload = json!({
        "cmd": "request.get",
        "url": url,
        "maxTimeout": 60000,
    });

    let response = ureq::post(flaresolverr_url)
        .set("Content-Type", "application/json")
        .send_json(serde_json::to_value(payload)?)?;

    // Deserialize and check for errors in the response
    let body: FlareSolverrResponse = response.into_json()?;
    if body.status != "ok" {
        return Err(format!("FlareSolverr error: {}", body.message).into());
    }

    let user_agent = body.solution.userAgent.clone();
    let store = convert_flaresolverr_cookies_to_ureq_cookies(CookieStore::default(), body.solution.cookies);

    let agent = build_ureq_agent(Some(&user_agent), Some(store));

    Ok(agent)
}

#[cfg(test)]
mod test {
    use std::env;

    use super::*;

    fn get_flaresolverr_response(url: &str, flaresolverr_url: &str) -> FlareSolverrResponse {

        let payload = json!({
            "cmd": "request.get",
            "url": url,
            "maxTimeout": 60000,
        });

        let flare_response = ureq::post(flaresolverr_url)
            .set("Content-Type", "application/json")
            .send_json(serde_json::to_value(payload).unwrap());

        assert!(flare_response.is_ok());

        let flare_body: FlareSolverrResponse = flare_response.unwrap().into_json().unwrap();

        flare_body
    }

    fn get_ureq_response(url: &str, flaresolverr_url: &str) -> String {
        let client = build_flaresolverr_client(url, flaresolverr_url).unwrap();

        let ureq_call = client.get(url);

        let ureq_response = ureq_call.call(); 

        if let Err(e) = &ureq_response {
            eprintln!("Error making request: {}", e);
        }

        assert!(ureq_response.is_ok());

        let ureq_body = ureq_response.unwrap().into_string().unwrap();

        ureq_body
    }

    #[test]
    #[ignore]
    fn test_nowsecure() {
        let flaresolverr_url = env::var("FLARESOLVERR_URL").unwrap_or_else(|_| "http://localhost:8191/v1".to_string());

        let flare_body = get_flaresolverr_response("https://nowsecure.com", &flaresolverr_url);
        assert!(!flare_body.solution.response.is_empty());

        let ureq_body = get_ureq_response("https://nowsecure.com", &flaresolverr_url);
        assert!(!ureq_body.is_empty());
    }

    #[test]
    #[ignore]
    fn test_openai() {
        let flaresolverr_url = env::var("FLARESOLVERR_URL").unwrap_or_else(|_| "http://localhost:8191/v1".to_string());

        let flare_body = get_flaresolverr_response("https://openai.com", &flaresolverr_url);
        assert!(!flare_body.solution.response.is_empty());

        let ureq_body = get_ureq_response("https://openai.com", &flaresolverr_url);
        assert!(!ureq_body.is_empty());
    }
    
}