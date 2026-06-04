// Copyright (c) 2024 ByoriDB
//
// This source code is licensed under Apache 2.0 License.

//! gRPC service implementation for Meta service
//!
//! This module implements the MetaService gRPC interface defined in meta.proto.
//! It wraps the underlying MetaService and handles proto type conversions.

use crate::error::MetaError;
use crate::proto::meta_service_server::MetaService as MetaServiceTrait;
use crate::proto::*;
use crate::schema::{
    self, AlterOperation as SchemaAlterOperation, DataType as SchemaDataType,
    VidType as SchemaVidType,
};
use crate::service::MetaService;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use tracing::{debug, info, warn};

/// gRPC Meta service implementation
pub struct MetaRpcService {
    service: Arc<MetaService>,
    /// Host address for leader hint (in single-node mode, this is self)
    host: HostAddr,
}

impl MetaRpcService {
    pub fn new(service: Arc<MetaService>, host: String, port: u32) -> Self {
        Self {
            service,
            host: HostAddr { host, port },
        }
    }

    fn to_error_code(err: &MetaError) -> ErrorCode {
        match err {
            MetaError::SpaceNotFound(_) => ErrorCode::ESpaceNotFound,
            MetaError::SpaceAlreadyExists(_) => ErrorCode::ESpaceAlreadyExists,
            MetaError::TagNotFound(_) => ErrorCode::ETagNotFound,
            MetaError::TagAlreadyExists(_) => ErrorCode::ETagAlreadyExists,
            MetaError::EdgeNotFound(_) => ErrorCode::EEdgeNotFound,
            MetaError::EdgeAlreadyExists(_) => ErrorCode::EEdgeAlreadyExists,
            MetaError::IndexNotFound(_) => ErrorCode::EIndexNotFound,
            MetaError::IndexAlreadyExists(_) => ErrorCode::EIndexAlreadyExists,
            MetaError::FieldNotFound(_) => ErrorCode::EFieldNotFound,
            MetaError::PartitionNotFound(_, _) => ErrorCode::EPartitionNotFound,
            MetaError::FieldAlreadyExists(_) => ErrorCode::EFieldAlreadyExists,
            MetaError::InvalidAlterOperation(_) => ErrorCode::EInvalidAlterOperation,
            _ => ErrorCode::EInternalError,
        }
    }

    fn convert_vid_type(vt: VidType) -> SchemaVidType {
        match vt {
            VidType::VidInt64 => SchemaVidType::Int64,
            VidType::VidFixedString => SchemaVidType::FixedString(32), // Default size
        }
    }

    fn convert_vid_type_to_proto(vt: &SchemaVidType) -> VidType {
        match vt {
            SchemaVidType::Int64 => VidType::VidInt64,
            SchemaVidType::FixedString(_) => VidType::VidFixedString,
        }
    }

    fn convert_data_type(dt: DataType) -> SchemaDataType {
        match dt {
            DataType::DtBool => SchemaDataType::Bool,
            DataType::DtInt8 => SchemaDataType::Int8,
            DataType::DtInt16 => SchemaDataType::Int16,
            DataType::DtInt32 => SchemaDataType::Int32,
            DataType::DtInt64 => SchemaDataType::Int64,
            DataType::DtFloat => SchemaDataType::Float,
            DataType::DtDouble => SchemaDataType::Double,
            DataType::DtString => SchemaDataType::String,
            DataType::DtFixedString => SchemaDataType::FixedString(32),
            DataType::DtTimestamp => SchemaDataType::Timestamp,
            DataType::DtDate => SchemaDataType::Date,
            DataType::DtTime => SchemaDataType::Time,
            DataType::DtDatetime => SchemaDataType::DateTime,
            DataType::DtGeography => SchemaDataType::Geography,
        }
    }

    fn convert_data_type_to_proto(dt: &SchemaDataType) -> DataType {
        match dt {
            SchemaDataType::Bool => DataType::DtBool,
            SchemaDataType::Int8 => DataType::DtInt8,
            SchemaDataType::Int16 => DataType::DtInt16,
            SchemaDataType::Int32 => DataType::DtInt32,
            SchemaDataType::Int64 => DataType::DtInt64,
            SchemaDataType::Float => DataType::DtFloat,
            SchemaDataType::Double => DataType::DtDouble,
            SchemaDataType::String => DataType::DtString,
            SchemaDataType::FixedString(_) => DataType::DtFixedString,
            SchemaDataType::Timestamp => DataType::DtTimestamp,
            SchemaDataType::Date => DataType::DtDate,
            SchemaDataType::Time => DataType::DtTime,
            SchemaDataType::DateTime => DataType::DtDatetime,
            SchemaDataType::Geography => DataType::DtGeography,
        }
    }

    fn convert_field_from_proto(f: &Field) -> schema::Field {
        let data_type = DataType::try_from(f.data_type).unwrap_or_else(|_| {
            tracing::warn!(
                field = %f.name,
                raw_type = f.data_type,
                "Unknown DataType in proto field, defaulting to String"
            );
            DataType::DtString
        });
        schema::Field {
            name: f.name.clone(),
            data_type: Self::convert_data_type(data_type),
            nullable: f.nullable,
            default: if f.default_value.is_empty() {
                None
            } else {
                Some(f.default_value.clone())
            },
        }
    }

    fn convert_fields(fields: &[Field]) -> Vec<schema::Field> {
        fields.iter().map(Self::convert_field_from_proto).collect()
    }

    fn convert_fields_to_proto(fields: &[schema::Field]) -> Vec<Field> {
        fields
            .iter()
            .map(|f| Field {
                name: f.name.clone(),
                data_type: Self::convert_data_type_to_proto(&f.data_type) as i32,
                nullable: f.nullable,
                default_value: f.default.clone().unwrap_or_default(),
                fixed_string_len: match &f.data_type {
                    SchemaDataType::FixedString(len) => *len as u32,
                    _ => 0,
                },
            })
            .collect()
    }

    #[allow(clippy::result_large_err)]
    fn convert_alter_operations(
        operations: &[AlterOperation],
    ) -> Result<Vec<SchemaAlterOperation>, Status> {
        operations
            .iter()
            .map(|op| {
                let op_type = AlterOperationType::try_from(op.op_type).map_err(|_| {
                    Status::invalid_argument(format!("Invalid AlterOperationType: {}", op.op_type))
                })?;
                match op_type {
                    AlterOperationType::AddColumn => {
                        let f = op.field.as_ref().ok_or_else(|| {
                            Status::invalid_argument("Missing field for AddColumn operation")
                        })?;
                        Ok(SchemaAlterOperation::AddColumn(
                            Self::convert_field_from_proto(f),
                        ))
                    }
                    AlterOperationType::DropColumn => {
                        Ok(SchemaAlterOperation::DropColumn(op.field_name.clone()))
                    }
                    AlterOperationType::ChangeColumn => {
                        let f = op.field.as_ref().ok_or_else(|| {
                            Status::invalid_argument("Missing field for ChangeColumn operation")
                        })?;
                        Ok(SchemaAlterOperation::ChangeColumn(
                            Self::convert_field_from_proto(f),
                        ))
                    }
                }
            })
            .collect()
    }

    /// Convert proto PartitionStrategy to byoridb_common::PartitionStrategy
    fn convert_partition_strategy(
        strategy: Option<PartitionStrategy>,
    ) -> byoridb_common::PartitionStrategy {
        match strategy {
            Some(s) => {
                let strategy_type = PartitionStrategyType::try_from(s.strategy_type)
                    .unwrap_or(PartitionStrategyType::PsHash);
                match strategy_type {
                    PartitionStrategyType::PsHash => byoridb_common::PartitionStrategy::Hash,
                    PartitionStrategyType::PsRange => byoridb_common::PartitionStrategy::Range {
                        boundaries: s.range_boundaries,
                    },
                    PartitionStrategyType::PsModulo => byoridb_common::PartitionStrategy::Modulo,
                }
            }
            None => byoridb_common::PartitionStrategy::Hash,
        }
    }

    /// Convert byoridb_common::PartitionStrategy to proto PartitionStrategy
    fn convert_partition_strategy_to_proto(
        strategy: &byoridb_common::PartitionStrategy,
    ) -> PartitionStrategy {
        match strategy {
            byoridb_common::PartitionStrategy::Hash => PartitionStrategy {
                strategy_type: PartitionStrategyType::PsHash as i32,
                range_boundaries: vec![],
            },
            byoridb_common::PartitionStrategy::Range { boundaries } => PartitionStrategy {
                strategy_type: PartitionStrategyType::PsRange as i32,
                range_boundaries: boundaries.clone(),
            },
            byoridb_common::PartitionStrategy::Modulo => PartitionStrategy {
                strategy_type: PartitionStrategyType::PsModulo as i32,
                range_boundaries: vec![],
            },
        }
    }
}

#[tonic::async_trait]
impl MetaServiceTrait for MetaRpcService {
    // ===== Space Operations =====

    async fn create_space(
        &self,
        request: Request<CreateSpaceRequest>,
    ) -> Result<Response<CreateSpaceResponse>, Status> {
        let req = request.into_inner();
        debug!("CreateSpace request: {:?}", req.space_name);

        let vid_type =
            Self::convert_vid_type(VidType::try_from(req.vid_type).unwrap_or(VidType::VidInt64));
        let partition_strategy = Self::convert_partition_strategy(req.partition_strategy);

        match self
            .service
            .create_space(
                req.space_name,
                req.partition_num,
                req.replica_factor,
                vid_type,
                partition_strategy,
            )
            .await
        {
            Ok(space_id) => {
                info!("Created space with ID {}", space_id);
                Ok(Response::new(CreateSpaceResponse {
                    code: ErrorCode::Succeeded as i32,
                    space_id,
                    leader: Some(self.host.clone()),
                }))
            }
            Err(e) => {
                warn!("CreateSpace failed: {}", e);
                Ok(Response::new(CreateSpaceResponse {
                    code: Self::to_error_code(&e) as i32,
                    space_id: 0,
                    leader: Some(self.host.clone()),
                }))
            }
        }
    }

    async fn get_space(
        &self,
        request: Request<GetSpaceRequest>,
    ) -> Result<Response<GetSpaceResponse>, Status> {
        let req = request.into_inner();
        debug!("GetSpace request: {:?}", req.space_name);

        match self.service.get_space_by_name(&req.space_name).await {
            Ok(space) => Ok(Response::new(GetSpaceResponse {
                code: ErrorCode::Succeeded as i32,
                space: Some(SpaceDesc {
                    space_id: space.id,
                    space_name: space.name,
                    partition_num: space.partition_num,
                    replica_factor: space.replica_factor,
                    vid_type: Self::convert_vid_type_to_proto(&space.vid_type) as i32,
                    vid_size: match space.vid_type {
                        SchemaVidType::FixedString(len) => len as u32,
                        _ => 0,
                    },
                    partition_strategy: Some(Self::convert_partition_strategy_to_proto(
                        &space.partition_strategy,
                    )),
                }),
                leader: Some(self.host.clone()),
            })),
            Err(e) => Ok(Response::new(GetSpaceResponse {
                code: Self::to_error_code(&e) as i32,
                space: None,
                leader: Some(self.host.clone()),
            })),
        }
    }

    async fn list_spaces(
        &self,
        _request: Request<ListSpacesRequest>,
    ) -> Result<Response<ListSpacesResponse>, Status> {
        debug!("ListSpaces request");

        match self.service.list_spaces().await {
            Ok(spaces) => {
                let space_descs: Vec<SpaceDesc> = spaces
                    .iter()
                    .map(|s| SpaceDesc {
                        space_id: s.id,
                        space_name: s.name.clone(),
                        partition_num: s.partition_num,
                        replica_factor: s.replica_factor,
                        vid_type: Self::convert_vid_type_to_proto(&s.vid_type) as i32,
                        vid_size: match &s.vid_type {
                            SchemaVidType::FixedString(len) => *len as u32,
                            _ => 0,
                        },
                        partition_strategy: Some(Self::convert_partition_strategy_to_proto(
                            &s.partition_strategy,
                        )),
                    })
                    .collect();

                Ok(Response::new(ListSpacesResponse {
                    code: ErrorCode::Succeeded as i32,
                    spaces: space_descs,
                    leader: Some(self.host.clone()),
                }))
            }
            Err(e) => Ok(Response::new(ListSpacesResponse {
                code: Self::to_error_code(&e) as i32,
                spaces: vec![],
                leader: Some(self.host.clone()),
            })),
        }
    }

    async fn drop_space(
        &self,
        request: Request<DropSpaceRequest>,
    ) -> Result<Response<DropSpaceResponse>, Status> {
        let req = request.into_inner();
        debug!("DropSpace request: {:?}", req.space_name);

        match self.service.drop_space(&req.space_name).await {
            Ok(()) => {
                info!("Dropped space: {}", req.space_name);
                Ok(Response::new(DropSpaceResponse {
                    code: ErrorCode::Succeeded as i32,
                    leader: Some(self.host.clone()),
                }))
            }
            Err(e) => Ok(Response::new(DropSpaceResponse {
                code: Self::to_error_code(&e) as i32,
                leader: Some(self.host.clone()),
            })),
        }
    }

    // ===== Tag Operations =====

    async fn create_tag(
        &self,
        request: Request<CreateTagRequest>,
    ) -> Result<Response<CreateTagResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "CreateTag request: {} in space {}",
            req.tag_name, req.space_id
        );

        let fields = Self::convert_fields(&req.fields);

        match self
            .service
            .create_tag(req.space_id, req.tag_name.clone(), fields)
            .await
        {
            Ok(tag_id) => {
                info!("Created tag {} with ID {}", req.tag_name, tag_id);
                Ok(Response::new(CreateTagResponse {
                    code: ErrorCode::Succeeded as i32,
                    tag_id,
                    leader: Some(self.host.clone()),
                }))
            }
            Err(e) => Ok(Response::new(CreateTagResponse {
                code: Self::to_error_code(&e) as i32,
                tag_id: 0,
                leader: Some(self.host.clone()),
            })),
        }
    }

    async fn get_tag(
        &self,
        request: Request<GetTagRequest>,
    ) -> Result<Response<GetTagResponse>, Status> {
        let req = request.into_inner();
        debug!("GetTag request: {} in space {}", req.tag_name, req.space_id);

        match self.service.get_tag(req.space_id, &req.tag_name).await {
            Ok(tag) => Ok(Response::new(GetTagResponse {
                code: ErrorCode::Succeeded as i32,
                tag: Some(TagDesc {
                    tag_id: tag.id,
                    space_id: tag.space_id,
                    tag_name: tag.name,
                    version: tag.version,
                    fields: Self::convert_fields_to_proto(&tag.fields),
                }),
                leader: Some(self.host.clone()),
            })),
            Err(e) => Ok(Response::new(GetTagResponse {
                code: Self::to_error_code(&e) as i32,
                tag: None,
                leader: Some(self.host.clone()),
            })),
        }
    }

    async fn list_tags(
        &self,
        request: Request<ListTagsRequest>,
    ) -> Result<Response<ListTagsResponse>, Status> {
        let req = request.into_inner();
        debug!("ListTags request for space {}", req.space_id);

        match self.service.list_tags(req.space_id).await {
            Ok(tags) => {
                let tag_descs: Vec<TagDesc> = tags
                    .iter()
                    .map(|t| TagDesc {
                        tag_id: t.id,
                        space_id: t.space_id,
                        tag_name: t.name.clone(),
                        version: t.version,
                        fields: Self::convert_fields_to_proto(&t.fields),
                    })
                    .collect();

                Ok(Response::new(ListTagsResponse {
                    code: ErrorCode::Succeeded as i32,
                    tags: tag_descs,
                    leader: Some(self.host.clone()),
                }))
            }
            Err(e) => Ok(Response::new(ListTagsResponse {
                code: Self::to_error_code(&e) as i32,
                tags: vec![],
                leader: Some(self.host.clone()),
            })),
        }
    }

    async fn drop_tag(
        &self,
        request: Request<DropTagRequest>,
    ) -> Result<Response<DropTagResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "DropTag request: {} in space {}",
            req.tag_name, req.space_id
        );

        match self.service.drop_tag(req.space_id, &req.tag_name).await {
            Ok(()) => Ok(Response::new(DropTagResponse {
                code: ErrorCode::Succeeded as i32,
                leader: Some(self.host.clone()),
            })),
            Err(e) => Ok(Response::new(DropTagResponse {
                code: Self::to_error_code(&e) as i32,
                leader: Some(self.host.clone()),
            })),
        }
    }

    async fn alter_tag(
        &self,
        request: Request<AlterTagRequest>,
    ) -> Result<Response<AlterTagResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "AlterTag request: {} in space {}",
            req.tag_name, req.space_id
        );

        let operations = Self::convert_alter_operations(&req.operations)?;

        match self
            .service
            .alter_tag(req.space_id, &req.tag_name, operations)
            .await
        {
            Ok(new_version) => {
                info!("Altered tag {}, new version {}", req.tag_name, new_version);
                Ok(Response::new(AlterTagResponse {
                    code: ErrorCode::Succeeded as i32,
                    new_version,
                    leader: Some(self.host.clone()),
                }))
            }
            Err(e) => {
                warn!("AlterTag failed: {}", e);
                Ok(Response::new(AlterTagResponse {
                    code: Self::to_error_code(&e) as i32,
                    new_version: 0,
                    leader: Some(self.host.clone()),
                }))
            }
        }
    }

    async fn get_tag_versions(
        &self,
        request: Request<GetTagVersionsRequest>,
    ) -> Result<Response<GetTagVersionsResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "GetTagVersions request: {} in space {}",
            req.tag_name, req.space_id
        );

        match self
            .service
            .get_tag_versions(req.space_id, &req.tag_name)
            .await
        {
            Ok(versions) => {
                let tag_descs: Vec<TagDesc> = versions
                    .iter()
                    .map(|t| TagDesc {
                        tag_id: t.id,
                        space_id: t.space_id,
                        tag_name: t.name.clone(),
                        version: t.version,
                        fields: Self::convert_fields_to_proto(&t.fields),
                    })
                    .collect();

                Ok(Response::new(GetTagVersionsResponse {
                    code: ErrorCode::Succeeded as i32,
                    versions: tag_descs,
                    leader: Some(self.host.clone()),
                }))
            }
            Err(e) => Ok(Response::new(GetTagVersionsResponse {
                code: Self::to_error_code(&e) as i32,
                versions: vec![],
                leader: Some(self.host.clone()),
            })),
        }
    }

    // ===== Edge Operations =====

    async fn create_edge(
        &self,
        request: Request<CreateEdgeRequest>,
    ) -> Result<Response<CreateEdgeResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "CreateEdge request: {} in space {}",
            req.edge_name, req.space_id
        );

        let fields = Self::convert_fields(&req.fields);

        match self
            .service
            .create_edge(req.space_id, req.edge_name.clone(), fields)
            .await
        {
            Ok(edge_id) => {
                info!("Created edge {} with ID {}", req.edge_name, edge_id);
                Ok(Response::new(CreateEdgeResponse {
                    code: ErrorCode::Succeeded as i32,
                    edge_id,
                    leader: Some(self.host.clone()),
                }))
            }
            Err(e) => Ok(Response::new(CreateEdgeResponse {
                code: Self::to_error_code(&e) as i32,
                edge_id: 0,
                leader: Some(self.host.clone()),
            })),
        }
    }

    async fn get_edge(
        &self,
        request: Request<GetEdgeRequest>,
    ) -> Result<Response<GetEdgeResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "GetEdge request: {} in space {}",
            req.edge_name, req.space_id
        );

        match self.service.get_edge(req.space_id, &req.edge_name).await {
            Ok(edge) => Ok(Response::new(GetEdgeResponse {
                code: ErrorCode::Succeeded as i32,
                edge: Some(EdgeDesc {
                    edge_id: edge.id,
                    space_id: edge.space_id,
                    edge_name: edge.name,
                    version: edge.version,
                    fields: Self::convert_fields_to_proto(&edge.fields),
                }),
                leader: Some(self.host.clone()),
            })),
            Err(e) => Ok(Response::new(GetEdgeResponse {
                code: Self::to_error_code(&e) as i32,
                edge: None,
                leader: Some(self.host.clone()),
            })),
        }
    }

    async fn list_edges(
        &self,
        request: Request<ListEdgesRequest>,
    ) -> Result<Response<ListEdgesResponse>, Status> {
        let req = request.into_inner();
        debug!("ListEdges request for space {}", req.space_id);

        match self.service.list_edges(req.space_id).await {
            Ok(edges) => {
                let edge_descs: Vec<EdgeDesc> = edges
                    .iter()
                    .map(|e| EdgeDesc {
                        edge_id: e.id,
                        space_id: e.space_id,
                        edge_name: e.name.clone(),
                        version: e.version,
                        fields: Self::convert_fields_to_proto(&e.fields),
                    })
                    .collect();

                Ok(Response::new(ListEdgesResponse {
                    code: ErrorCode::Succeeded as i32,
                    edges: edge_descs,
                    leader: Some(self.host.clone()),
                }))
            }
            Err(e) => Ok(Response::new(ListEdgesResponse {
                code: Self::to_error_code(&e) as i32,
                edges: vec![],
                leader: Some(self.host.clone()),
            })),
        }
    }

    async fn drop_edge(
        &self,
        request: Request<DropEdgeRequest>,
    ) -> Result<Response<DropEdgeResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "DropEdge request: {} in space {}",
            req.edge_name, req.space_id
        );

        match self.service.drop_edge(req.space_id, &req.edge_name).await {
            Ok(()) => Ok(Response::new(DropEdgeResponse {
                code: ErrorCode::Succeeded as i32,
                leader: Some(self.host.clone()),
            })),
            Err(e) => Ok(Response::new(DropEdgeResponse {
                code: Self::to_error_code(&e) as i32,
                leader: Some(self.host.clone()),
            })),
        }
    }

    async fn alter_edge(
        &self,
        request: Request<AlterEdgeRequest>,
    ) -> Result<Response<AlterEdgeResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "AlterEdge request: {} in space {}",
            req.edge_name, req.space_id
        );

        let operations = Self::convert_alter_operations(&req.operations)?;

        match self
            .service
            .alter_edge(req.space_id, &req.edge_name, operations)
            .await
        {
            Ok(new_version) => {
                info!(
                    "Altered edge {}, new version {}",
                    req.edge_name, new_version
                );
                Ok(Response::new(AlterEdgeResponse {
                    code: ErrorCode::Succeeded as i32,
                    new_version,
                    leader: Some(self.host.clone()),
                }))
            }
            Err(e) => {
                warn!("AlterEdge failed: {}", e);
                Ok(Response::new(AlterEdgeResponse {
                    code: Self::to_error_code(&e) as i32,
                    new_version: 0,
                    leader: Some(self.host.clone()),
                }))
            }
        }
    }

    async fn get_edge_versions(
        &self,
        request: Request<GetEdgeVersionsRequest>,
    ) -> Result<Response<GetEdgeVersionsResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "GetEdgeVersions request: {} in space {}",
            req.edge_name, req.space_id
        );

        match self
            .service
            .get_edge_versions(req.space_id, &req.edge_name)
            .await
        {
            Ok(versions) => {
                let edge_descs: Vec<EdgeDesc> = versions
                    .iter()
                    .map(|e| EdgeDesc {
                        edge_id: e.id,
                        space_id: e.space_id,
                        edge_name: e.name.clone(),
                        version: e.version,
                        fields: Self::convert_fields_to_proto(&e.fields),
                    })
                    .collect();

                Ok(Response::new(GetEdgeVersionsResponse {
                    code: ErrorCode::Succeeded as i32,
                    versions: edge_descs,
                    leader: Some(self.host.clone()),
                }))
            }
            Err(e) => Ok(Response::new(GetEdgeVersionsResponse {
                code: Self::to_error_code(&e) as i32,
                versions: vec![],
                leader: Some(self.host.clone()),
            })),
        }
    }

    // ===== Tag Index Operations =====

    async fn create_tag_index(
        &self,
        request: Request<CreateTagIndexRequest>,
    ) -> Result<Response<CreateTagIndexResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "CreateTagIndex request: {} on {} in space {}",
            req.index_name, req.tag_name, req.space_id
        );

        match self
            .service
            .create_tag_index(
                req.space_id,
                req.index_name.clone(),
                req.tag_name,
                req.fields,
            )
            .await
        {
            Ok(index_id) => {
                info!("Created tag index {} with ID {}", req.index_name, index_id);
                Ok(Response::new(CreateTagIndexResponse {
                    code: ErrorCode::Succeeded as i32,
                    index_id,
                    leader: Some(self.host.clone()),
                }))
            }
            Err(e) => Ok(Response::new(CreateTagIndexResponse {
                code: Self::to_error_code(&e) as i32,
                index_id: 0,
                leader: Some(self.host.clone()),
            })),
        }
    }

    async fn list_tag_indexes(
        &self,
        request: Request<ListTagIndexesRequest>,
    ) -> Result<Response<ListTagIndexesResponse>, Status> {
        let req = request.into_inner();
        debug!("ListTagIndexes request for space {}", req.space_id);

        match self.service.list_tag_indexes(req.space_id).await {
            Ok(indexes) => {
                let index_descs: Vec<TagIndexDesc> = indexes
                    .iter()
                    .map(|i| TagIndexDesc {
                        index_id: i.id,
                        space_id: i.space_id,
                        index_name: i.index_name.clone(),
                        tag_id: i.tag_id,
                        fields: i.fields.clone(),
                    })
                    .collect();

                Ok(Response::new(ListTagIndexesResponse {
                    code: ErrorCode::Succeeded as i32,
                    indexes: index_descs,
                    leader: Some(self.host.clone()),
                }))
            }
            Err(e) => Ok(Response::new(ListTagIndexesResponse {
                code: Self::to_error_code(&e) as i32,
                indexes: vec![],
                leader: Some(self.host.clone()),
            })),
        }
    }

    async fn drop_tag_index(
        &self,
        request: Request<DropTagIndexRequest>,
    ) -> Result<Response<DropTagIndexResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "DropTagIndex request: {} in space {}",
            req.index_name, req.space_id
        );

        match self
            .service
            .drop_tag_index(req.space_id, &req.index_name)
            .await
        {
            Ok(()) => Ok(Response::new(DropTagIndexResponse {
                code: ErrorCode::Succeeded as i32,
                leader: Some(self.host.clone()),
            })),
            Err(e) => Ok(Response::new(DropTagIndexResponse {
                code: Self::to_error_code(&e) as i32,
                leader: Some(self.host.clone()),
            })),
        }
    }

    // ===== Edge Index Operations =====

    async fn create_edge_index(
        &self,
        request: Request<CreateEdgeIndexRequest>,
    ) -> Result<Response<CreateEdgeIndexResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "CreateEdgeIndex request: {} on {} in space {}",
            req.index_name, req.edge_name, req.space_id
        );

        match self
            .service
            .create_edge_index(
                req.space_id,
                req.index_name.clone(),
                req.edge_name,
                req.fields,
            )
            .await
        {
            Ok(index_id) => {
                info!("Created edge index {} with ID {}", req.index_name, index_id);
                Ok(Response::new(CreateEdgeIndexResponse {
                    code: ErrorCode::Succeeded as i32,
                    index_id,
                    leader: Some(self.host.clone()),
                }))
            }
            Err(e) => Ok(Response::new(CreateEdgeIndexResponse {
                code: Self::to_error_code(&e) as i32,
                index_id: 0,
                leader: Some(self.host.clone()),
            })),
        }
    }

    async fn list_edge_indexes(
        &self,
        request: Request<ListEdgeIndexesRequest>,
    ) -> Result<Response<ListEdgeIndexesResponse>, Status> {
        let req = request.into_inner();
        debug!("ListEdgeIndexes request for space {}", req.space_id);

        match self.service.list_edge_indexes(req.space_id).await {
            Ok(indexes) => {
                let index_descs: Vec<EdgeIndexDesc> = indexes
                    .iter()
                    .map(|i| EdgeIndexDesc {
                        index_id: i.id,
                        space_id: i.space_id,
                        index_name: i.index_name.clone(),
                        edge_id: i.edge_type,
                        fields: i.fields.clone(),
                    })
                    .collect();

                Ok(Response::new(ListEdgeIndexesResponse {
                    code: ErrorCode::Succeeded as i32,
                    indexes: index_descs,
                    leader: Some(self.host.clone()),
                }))
            }
            Err(e) => Ok(Response::new(ListEdgeIndexesResponse {
                code: Self::to_error_code(&e) as i32,
                indexes: vec![],
                leader: Some(self.host.clone()),
            })),
        }
    }

    async fn drop_edge_index(
        &self,
        request: Request<DropEdgeIndexRequest>,
    ) -> Result<Response<DropEdgeIndexResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "DropEdgeIndex request: {} in space {}",
            req.index_name, req.space_id
        );

        match self
            .service
            .drop_edge_index(req.space_id, &req.index_name)
            .await
        {
            Ok(()) => Ok(Response::new(DropEdgeIndexResponse {
                code: ErrorCode::Succeeded as i32,
                leader: Some(self.host.clone()),
            })),
            Err(e) => Ok(Response::new(DropEdgeIndexResponse {
                code: Self::to_error_code(&e) as i32,
                leader: Some(self.host.clone()),
            })),
        }
    }

    // ===== Partition Operations =====

    async fn get_parts_alloc(
        &self,
        request: Request<GetPartsAllocRequest>,
    ) -> Result<Response<GetPartsAllocResponse>, Status> {
        let req = request.into_inner();
        debug!("GetPartsAlloc request for space {}", req.space_id);

        match self.service.get_parts_alloc(req.space_id).await {
            Ok(parts) => {
                let part_allocs: Vec<PartAlloc> = parts
                    .iter()
                    .map(|p| PartAlloc {
                        space_id: p.space_id,
                        part_id: p.part_id,
                        hosts: p
                            .hosts
                            .iter()
                            .map(|(h, p)| HostAddr {
                                host: h.clone(),
                                port: *p,
                            })
                            .collect(),
                    })
                    .collect();

                Ok(Response::new(GetPartsAllocResponse {
                    code: ErrorCode::Succeeded as i32,
                    parts: part_allocs,
                    leader: Some(self.host.clone()),
                }))
            }
            Err(e) => Ok(Response::new(GetPartsAllocResponse {
                code: Self::to_error_code(&e) as i32,
                parts: vec![],
                leader: Some(self.host.clone()),
            })),
        }
    }

    async fn get_part_hosts(
        &self,
        request: Request<GetPartHostsRequest>,
    ) -> Result<Response<GetPartHostsResponse>, Status> {
        let req = request.into_inner();
        debug!(
            "GetPartHosts request for space {} part {}",
            req.space_id, req.part_id
        );

        match self.service.get_part_hosts(req.space_id, req.part_id).await {
            Ok(hosts) => {
                let host_addrs: Vec<HostAddr> = hosts
                    .iter()
                    .map(|(h, p)| HostAddr {
                        host: h.clone(),
                        port: *p,
                    })
                    .collect();

                Ok(Response::new(GetPartHostsResponse {
                    code: ErrorCode::Succeeded as i32,
                    hosts: host_addrs,
                    leader: Some(self.host.clone()),
                }))
            }
            Err(e) => Ok(Response::new(GetPartHostsResponse {
                code: Self::to_error_code(&e) as i32,
                hosts: vec![],
                leader: Some(self.host.clone()),
            })),
        }
    }

    // ===== Host Operations =====

    async fn list_hosts(
        &self,
        _request: Request<ListHostsRequest>,
    ) -> Result<Response<ListHostsResponse>, Status> {
        debug!("ListHosts request");

        let summaries = self.service.list_hosts_with_counts();
        let items: Vec<HostItem> = summaries
            .into_iter()
            .map(|s| {
                let status = match s.status {
                    crate::service::HostStatus::Online => HostStatusProto::HsOnline,
                    crate::service::HostStatus::Offline => HostStatusProto::HsOffline,
                };
                HostItem {
                    host: Some(HostAddr {
                        host: s.host,
                        port: s.port,
                    }),
                    status: status as i32,
                    leader_count: s.leader_count,
                    part_count: s.part_count,
                }
            })
            .collect();

        Ok(Response::new(ListHostsResponse {
            code: ErrorCode::Succeeded as i32,
            hosts: items,
            leader: Some(self.host.clone()),
        }))
    }

    // ===== Heartbeat =====

    async fn heartbeat(
        &self,
        request: Request<HeartbeatRequest>,
    ) -> Result<Response<HeartbeatResponse>, Status> {
        let req = request.into_inner();
        let host_addr = req
            .host
            .ok_or_else(|| Status::invalid_argument("Missing host"))?;

        debug!(
            "Heartbeat from {}:{} role={}",
            host_addr.host, host_addr.port, req.role
        );

        match self.service.handle_heartbeat(
            host_addr.host,
            host_addr.port,
            &req.role,
            req.cluster_id,
        ) {
            Ok(info) => Ok(Response::new(HeartbeatResponse {
                code: ErrorCode::Succeeded as i32,
                leader: Some(self.host.clone()),
                cluster_id: info.cluster_id,
            })),
            Err(e) => {
                warn!("Heartbeat handling failed: {}", e);
                Ok(Response::new(HeartbeatResponse {
                    code: ErrorCode::EInternalError as i32,
                    leader: Some(self.host.clone()),
                    cluster_id: 0,
                }))
            }
        }
    }
}
