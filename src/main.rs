use askama::Template;
use askama_axum::Response;
use axum::body::{Body, Bytes};
use axum::extract::FromRequestParts;
use axum::extract::{Path, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect};
use axum::routing::get;
use axum::{async_trait, Router};
use dotenv::dotenv;
use reqwest::header::HeaderMap;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use axum_auth::{AuthBasic, AuthBasicCustom};


struct Config {
    port: u16,
    repos_config_path: String,
    github_token: Option<String>,
}

impl Config {
    fn from_env() -> Config {
        let port = std::env::var("SERVICE_PORT")
            .map(|v| {
                v.parse::<u16>()
                    .expect("cannot parse SERVICE_PORT env variable")
            })
            .or::<u16>(Ok(8000))
            .unwrap();
        let github_token = std::env::var("GITHUB_TOKEN").ok();
        let repos_config_path = std::env::var("REPOS_CONFIG_PATH")
            .or("repos.json".parse())
            .unwrap();

        return Config {
            port,
            repos_config_path,
            github_token,
        };
    }
}

struct GithubToken(Option<String>);

#[async_trait]
impl FromRequestParts<Arc<AppState>> for GithubToken {
    type Rejection = ErrorResponse;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let basic_auth = AuthBasic::decode_request_parts(parts);
        if let Ok(AuthBasic((_, Some(password)))) = basic_auth {
            return Ok(GithubToken(Some(password)))
        }
        if let Some(token) = &state.config.github_token {
            return Ok(GithubToken(Some(token.clone())));
        }
        Ok(GithubToken(None))
    }
}

#[derive(Template)]
#[template(path = "simple.html")]
pub struct Simple {
    repos: Vec<String>,
}

async fn simple(State(app_state): State<Arc<AppState>>) -> Simple {
    return Simple {
        repos: app_state.repos.all(),
    };
}

fn get_repository<'a>(
    package_name: &String,
    app_state: &'a AppState,
) -> Result<&'a Repository, ErrorResponse> {
    return app_state
        .repos
        .get(package_name)
        .ok_or(ErrorResponse::PageNotFound {});
}

#[derive(Template)]
#[template(path = "package.html")]
pub struct PackageTemplate {
    github_org: String,
    package_name: String,
    assets: Vec<Asset>,
}

async fn package(
    State(app_state): State<Arc<AppState>>,
    Path((package_name,)): Path<(String,)>,
    GithubToken(token): GithubToken,
) -> Result<PackageTemplate, ErrorResponse> {
    let client = GithubClient::new(token.clone());
    let package = get_repository(&package_name, &app_state)?;
    let assets = client.list_packages(&package.owner, &package.name).await?;
    return Ok(PackageTemplate {
        github_org: package.owner.clone(),
        assets,
        package_name,
    });
}

enum ErrorResponse {
    ServerError(Option<String>),
    PageNotFound,
}

impl From<reqwest::Error> for ErrorResponse {
    fn from(_value: reqwest::Error) -> Self {
        return ErrorResponse::ServerError(Some("Error during http request".to_string()));
    }
}

impl IntoResponse for ErrorResponse {
    fn into_response(self) -> Response {
        match self {
            ErrorResponse::ServerError(message) => {
                let message = message
                    .or(Some("Internal server error".to_string()))
                    .unwrap();
                (StatusCode::INTERNAL_SERVER_ERROR, message).into_response()
            }
            ErrorResponse::PageNotFound => (StatusCode::NOT_FOUND, "Page not found").into_response(),
        }
    }
}

#[derive(Deserialize)]
struct Release {
    assets: Vec<Asset>,
}

#[derive(Deserialize, Clone)]
struct Asset {
    id: u64,
    name: String,
}

struct GithubClient {
    client: reqwest::Client,
}

impl GithubClient {
    fn new(token: Option<String>) -> Self {
        let mut default_headers = HeaderMap::new();
        default_headers.insert(reqwest::header::USER_AGENT, "pigi".parse().unwrap());
        if let Some(token) = token {
            default_headers.insert(
                reqwest::header::AUTHORIZATION,
                format!("token {}", token).parse().unwrap(),
            );
        }
        default_headers.insert("X-GitHub-Api-Version", "2022-11-28".parse().unwrap());
        default_headers.insert(reqwest::header::ACCEPT,"application/vnd.github+json".parse().unwrap());

        let client = reqwest::Client::builder()
            .default_headers(default_headers)
            .build()
            .unwrap();
        return GithubClient { client };
    }
    async fn list_packages(
        self: &Self,
        org: &String,
        repo: &String,
    ) -> Result<Vec<Asset>, ErrorResponse> {
        let url = format!("https://api.github.com/repos/{}/{}/releases", org, repo);
        let response = self.client.get(url).send().await?;
        let data = response.json::<Vec<Release>>().await?;
        let results = data
            .iter()
            .flat_map(|release| release.assets.iter())
            .map(|asset| asset.clone())
            .collect();
        return Ok(results);
    }

    async fn asset(
        self: &Self,
        org: &String,
        repo: &String,
        asset_id: &String,
    ) -> Result<impl futures_core::Stream<Item = reqwest::Result<Bytes>>, ErrorResponse> {
        let url = format!(
            "https://api.github.com/repos/{}/{}/releases/assets/{}",
            org, repo, asset_id
        );

        let response = self
            .client
            .get(url)
            .header("Accept", "application/octet-stream")
            .send()
            .await?;

        return Ok(response.bytes_stream());
    }
}

async fn asset(
    State(app_state): State<Arc<AppState>>,
    Path((package_name, asset_id)): Path<(String, String)>,
    GithubToken(token): GithubToken,
) -> Result<Response, ErrorResponse> {
    let client = GithubClient::new(token);
    let repository = get_repository(&package_name, &app_state)?;

    let stream = client
        .asset(&repository.owner, &repository.name, &asset_id)
        .await?;
    return Ok(Body::from_stream(stream).into_response());
}

#[derive(Deserialize)]
struct Repository {
    owner: String,
    name: String,
}

struct AppState {
    config: Config,
    repos: Repositories,
}

#[derive(Deserialize)]
struct Repositories(HashMap<String, Repository>);

impl Repositories {
    fn from_config(config: &Config) -> Self {
        let json_content = fs::read_to_string(&config.repos_config_path)
            .expect("Failed to load repos config file");
        return serde_json::from_str(&json_content).expect("failed to process config file");
    }
    fn all(self: &Self) -> Vec<String> {
        return self.0.keys().map(|key| key.clone()).collect();
    }

    fn get(&self, name: &String) -> Option<&Repository> {
        return self.0.get(name);
    }
}

#[tokio::main]
async fn main() {
    dotenv().ok();

    let config = Config::from_env();
    let repos = Repositories::from_config(&config);
    let routes = Router::new()
        .route("/simple", get(|| async { Redirect::permanent("/simple/") }))
        .route("/simple/", get(simple))
        .route(
            "/simple/:package",
            get(|Path((package_name,)): Path<(String,)>| async move {
                Redirect::permanent(format!("/simple/{}/", package_name.clone()).as_str())
            }),
        )
        .route("/simple/:package/", get(package))
        .route("/simple/:package/:asset/:asset_name", get(asset));

    let host = format!("0.0.0.0:{}", config.port);
    let listener = tokio::net::TcpListener::bind(&host).await.unwrap();
    println!("Serving under: http://{}", host);
    let server = routes.with_state(Arc::new(AppState { config, repos }));
    axum::serve(listener, server).await.unwrap();
}
