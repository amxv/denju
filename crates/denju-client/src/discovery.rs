use denju_wire::{
    CatalogSearchQuery, CatalogSearchResponse, CatalogTopQuery, FollowMutationRequest,
    FollowMutationResponse, ProfileUpdateRequest, ProfileUpdateResponse, ReportResourceRequest,
    ReportResourceResponse, ResourceTopicsRequest, ResourceTopicsResponse, StarMutationRequest,
    StarMutationResponse, UniversalShowResponse,
};

use super::{ClientError, RegistryClient, decode_response};

impl RegistryClient {
    pub async fn search_catalog(
        &self,
        query: &CatalogSearchQuery,
    ) -> Result<CatalogSearchResponse, ClientError> {
        let request = self.http.get(self.endpoint("v1/search")?).query(query);
        let request = if self.bearer.is_some() {
            self.with_auth(request)?
        } else {
            request
        };
        decode_response(request.send().await?).await
    }

    pub async fn top_catalog(
        &self,
        query: &CatalogTopQuery,
    ) -> Result<CatalogSearchResponse, ClientError> {
        let request = self.http.get(self.endpoint("v1/top")?).query(query);
        let request = if self.bearer.is_some() {
            self.with_auth(request)?
        } else {
            request
        };
        decode_response(request.send().await?).await
    }

    pub async fn universal_show(
        &self,
        locator: &str,
        followers_cursor: Option<&str>,
        following_cursor: Option<&str>,
    ) -> Result<UniversalShowResponse, ClientError> {
        let mut request = self
            .http
            .get(self.endpoint("v1/show")?)
            .query(&[("locator", locator)]);
        if let Some(cursor) = followers_cursor {
            request = request.query(&[("followers_cursor", cursor)]);
        }
        if let Some(cursor) = following_cursor {
            request = request.query(&[("following_cursor", cursor)]);
        }
        let request = if self.bearer.is_some() {
            self.with_auth(request)?
        } else {
            request
        };
        decode_response(request.send().await?).await
    }

    pub async fn update_profile(
        &self,
        request: &ProfileUpdateRequest,
    ) -> Result<ProfileUpdateResponse, ClientError> {
        self.authenticated_post_json("v1/profile", request).await
    }

    pub async fn follow(
        &self,
        request: &FollowMutationRequest,
    ) -> Result<FollowMutationResponse, ClientError> {
        self.authenticated_post_json("v1/follows", request).await
    }

    pub async fn unfollow(
        &self,
        request: &FollowMutationRequest,
    ) -> Result<FollowMutationResponse, ClientError> {
        self.authenticated_post_json("v1/follows/remove", request)
            .await
    }

    pub async fn star(
        &self,
        request: &StarMutationRequest,
    ) -> Result<StarMutationResponse, ClientError> {
        self.authenticated_post_json("v1/stars", request).await
    }

    pub async fn unstar(
        &self,
        request: &StarMutationRequest,
    ) -> Result<StarMutationResponse, ClientError> {
        self.authenticated_post_json("v1/stars/remove", request)
            .await
    }

    pub async fn update_resource_topics(
        &self,
        request: &ResourceTopicsRequest,
    ) -> Result<ResourceTopicsResponse, ClientError> {
        self.authenticated_post_json("v1/resources/topics", request)
            .await
    }

    pub async fn report_resource(
        &self,
        request: &ReportResourceRequest,
    ) -> Result<ReportResourceResponse, ClientError> {
        self.authenticated_post_json("v1/reports", request).await
    }
}
