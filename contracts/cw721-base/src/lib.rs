pub mod error;
mod execute;
pub mod helpers;
pub mod msg;
mod query;
pub mod state;

#[cfg(test)]
mod contract_tests;
#[cfg(test)]
mod multi_tests;

pub use crate::error::ContractError;
pub use crate::msg::{ExecuteMsg, InstantiateMsg, MinterResponse, QueryMsg};
pub use crate::state::Cw721Contract;

// These types are re-exported so that contracts interacting with this
// one don't need a direct dependency on cw_ownable to use the API.
//
// `Action` is used in `ExecuteMsg::UpdateOwnership`, `Ownership` is
// used in `QueryMsg::Ownership`, and `OwnershipError` is used in
// `ContractError::Ownership`.
pub use cw_ownable::{Action, Ownership, OwnershipError};

use cosmwasm_std::Empty;

// These are simple types to let us handle empty extensions
pub use cw721::EmptyCollectionInfoExtension;
pub use cw721::EmptyExtension;

// Version info for migration
pub const CONTRACT_NAME: &str = "crates.io:cw721-base";
pub const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

pub mod entry {
    use self::{
        msg::MigrateMsg,
        state::{token_owner_idx, NftInfo, TokenIndexes, CREATOR, MINTER},
    };

    use super::*;

    #[cfg(not(feature = "library"))]
    use cosmwasm_std::entry_point;
    use cosmwasm_std::{
        Addr, Api, Binary, Deps, DepsMut, Env, MessageInfo, Order, Response, StdResult, Storage,
    };
    use cw721::CollectionInfo;
    use cw_ownable::none_or;
    use cw_storage_plus::{IndexedMap, Item, MultiIndex};

    // This makes a conscious choice on the various generics used by the contract
    #[cfg_attr(not(feature = "library"), entry_point)]
    pub fn instantiate(
        deps: DepsMut,
        env: Env,
        info: MessageInfo,
        msg: InstantiateMsg<EmptyCollectionInfoExtension>,
    ) -> Result<Response, ContractError> {
        cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

        let tract = Cw721Contract::<
            EmptyExtension,
            Empty,
            Empty,
            Empty,
            EmptyCollectionInfoExtension,
        >::default();
        tract.instantiate(deps, env, info, msg)
    }

    #[cfg_attr(not(feature = "library"), entry_point)]
    pub fn execute(
        deps: DepsMut,
        env: Env,
        info: MessageInfo,
        msg: ExecuteMsg<EmptyExtension, Empty>,
    ) -> Result<Response, ContractError> {
        let tract = Cw721Contract::<EmptyExtension, Empty, Empty, Empty, Empty>::default();
        tract.execute(deps, env, info, msg)
    }

    #[cfg_attr(not(feature = "library"), entry_point)]
    pub fn query(deps: Deps, env: Env, msg: QueryMsg<Empty>) -> StdResult<Binary> {
        let tract = Cw721Contract::<EmptyExtension, Empty, Empty, Empty, Empty>::default();
        tract.query(deps, env, msg)
    }

    #[cfg_attr(not(feature = "library"), entry_point)]
    pub fn migrate(deps: DepsMut, env: Env, msg: MigrateMsg) -> Result<Response, ContractError> {
        let response = Response::<Empty>::default();
        // first migrate legacy data ...
        let response =
            migrate_legacy_minter_and_creator(deps.storage, deps.api, &env, &msg, response)?;
        let response = migrate_legacy_collection_info(deps.storage, &env, &msg, response)?;
        let response = migrate_legacy_tokens(deps.storage, &env, &msg, response)?;
        // ... then migrate
        let response = migrate_version(deps.storage, &env, &msg, response)?;
        // ... and update creator and minter AFTER legacy migration
        let response = migrate_creator(deps.storage, deps.api, &env, &msg, response)?;
        let response = migrate_minter(deps.storage, deps.api, &env, &msg, response)?;
        Ok(response)
    }

    pub fn migrate_version(
        storage: &mut dyn Storage,
        _env: &Env,
        _msg: &MigrateMsg,
        response: Response,
    ) -> StdResult<Response> {
        let response = response
            .add_attribute("from_version", cw2::get_contract_version(storage)?.version)
            .add_attribute("to_version", CONTRACT_VERSION);

        // update contract version
        cw2::set_contract_version(storage, CONTRACT_NAME, CONTRACT_VERSION)?;
        Ok(response)
    }

    pub fn migrate_creator(
        storage: &mut dyn Storage,
        api: &dyn Api,
        _env: &Env,
        msg: &MigrateMsg,
        response: Response,
    ) -> StdResult<Response> {
        match msg {
            MigrateMsg::WithUpdate { creator, .. } => {
                if let Some(creator) = creator {
                    CREATOR.initialize_owner(storage, api, Some(creator.as_str()))?;
                    return Ok(response.add_attribute("creator", creator));
                }
            }
        }
        Ok(response)
    }

    pub fn migrate_minter(
        storage: &mut dyn Storage,
        api: &dyn Api,
        _env: &Env,
        msg: &MigrateMsg,
        response: Response,
    ) -> StdResult<Response> {
        match msg {
            MigrateMsg::WithUpdate { minter, .. } => {
                if let Some(minter) = minter {
                    MINTER.initialize_owner(storage, api, Some(minter.as_str()))?;
                    return Ok(response.add_attribute("creator", minter));
                }
            }
        }
        Ok(response)
    }

    /// Migrates only in case ownership is not present
    /// !!! Important note here: !!!
    /// - creator owns the contract and can update collection info
    /// - minter can mint new tokens
    ///
    /// Before v0.19.0 there were confusing naming conventions:
    /// - v0.17.0: minter was replaced by cw_ownable, as a result minter is owner
    /// - v0.16.0 and below: minter was stored in dedicated `minter` store (so NOT using cw_ownable at all)
    pub fn migrate_legacy_minter_and_creator(
        storage: &mut dyn Storage,
        api: &dyn Api,
        _env: &Env,
        _msg: &MigrateMsg,
        response: Response,
    ) -> Result<Response, ContractError> {
        let minter = MINTER.item.may_load(storage)?;
        // no migration in case minter is already set
        if minter.is_some() {
            return Ok(response);
        }
        // in v0.17/18 cw_ownable::OWNERSHIP was used for minter, now it is used for creator
        let ownership_previously_used_as_minter = CREATOR.item.may_load(storage)?;
        let creator_and_minter = match ownership_previously_used_as_minter {
            // v0.18 migration
            Some(ownership) => {
                // owner is used for both: creator and minter
                // since it is already set for creator, we only need to migrate minter
                let owner = ownership.owner.map(|a| a.to_string());
                MINTER.initialize_owner(storage, api, owner.as_deref())?;
                owner
            }
            // v0.17 and older migration
            None => {
                let legacy_minter_store: Item<Addr> = Item::new("minter");
                let legacy_minter = legacy_minter_store.load(storage)?;
                MINTER.initialize_owner(storage, api, Some(legacy_minter.as_str()))?;
                CREATOR.initialize_owner(storage, api, Some(legacy_minter.as_str()))?;
                Some(legacy_minter.to_string())
            }
        };
        Ok(response.add_attribute("creator_and_minter", none_or(creator_and_minter.as_ref())))
    }

    /// Migrates only in case collection_info is not present
    pub fn migrate_legacy_collection_info(
        storage: &mut dyn Storage,
        env: &Env,
        _msg: &MigrateMsg,
        response: Response,
    ) -> Result<Response, ContractError> {
        let contract = Cw721Contract::<
            EmptyExtension,
            Empty,
            Empty,
            Empty,
            EmptyCollectionInfoExtension,
        >::default();
        match contract.collection_info.may_load(storage)? {
            Some(_) => Ok(response),
            None => {
                // contract info is legacy collection info
                let legacy_collection_info_store: Item<cw721_016::ContractInfoResponse> =
                    Item::new("nft_info");
                let legacy_collection_info = legacy_collection_info_store.load(storage)?;
                let collection_info = CollectionInfo {
                    name: legacy_collection_info.name.clone(),
                    symbol: legacy_collection_info.symbol.clone(),
                    extension: None,
                    updated_at: env.block.time,
                };
                contract.collection_info.save(storage, &collection_info)?;
                Ok(response
                    .add_attribute("migrated collection name", legacy_collection_info.name)
                    .add_attribute("migrated collection symbol", legacy_collection_info.symbol))
            }
        }
    }

    /// Migrates only in case collection_info is not present
    pub fn migrate_legacy_tokens(
        storage: &mut dyn Storage,
        _env: &Env,
        _msg: &MigrateMsg,
        response: Response,
    ) -> StdResult<Response> {
        let contract = Cw721Contract::<EmptyExtension, Empty, Empty, Empty, Empty>::default();
        match contract.nft_info.is_empty(storage) {
            false => Ok(response),
            true => {
                let indexes = TokenIndexes {
                    owner: MultiIndex::new(token_owner_idx, "tokens", "tokens__owner"),
                };
                let legacy_tokens_store: IndexedMap<
                    &str,
                    NftInfo<EmptyExtension>,
                    TokenIndexes<EmptyExtension>,
                > = IndexedMap::new("tokens", indexes);
                let keys = legacy_tokens_store
                    .keys(storage, None, None, Order::Ascending)
                    .collect::<StdResult<Vec<String>>>()?;
                for key in keys {
                    let legacy_token = legacy_tokens_store.load(storage, &key)?;
                    contract.nft_info.save(storage, &key, &legacy_token)?;
                }
                Ok(response)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use cosmwasm_std::{
        testing::{mock_dependencies, mock_env, mock_info},
        Addr, Order, StdResult,
    };
    use cw2::ContractVersion;
    use cw721::{CollectionInfo, Cw721Query};
    use cw_storage_plus::{IndexedMap, Item, MultiIndex};

    use crate::{
        msg::MigrateMsg,
        query::MAX_LIMIT,
        state::{token_owner_idx, NftInfo, TokenIndexes, CREATOR, MINTER},
    };

    use super::*;

    /// Make sure cw2 version info is properly initialized during instantiation.
    #[test]
    fn proper_cw2_initialization() {
        let mut deps = mock_dependencies();

        entry::instantiate(
            deps.as_mut(),
            mock_env(),
            mock_info("larry", &[]),
            InstantiateMsg {
                name: "collection_name".into(),
                symbol: "collection_symbol".into(),
                collection_info_extension: None,
                minter: Some("minter".into()),
                creator: Some("creator".into()),
                withdraw_address: None,
            },
        )
        .unwrap();

        let minter = MINTER
            .get_ownership(deps.as_ref().storage)
            .unwrap()
            .owner
            .map(|a| a.into_string());
        assert_eq!(minter, Some("minter".to_string()));

        let creator = CREATOR
            .get_ownership(deps.as_ref().storage)
            .unwrap()
            .owner
            .map(|a| a.into_string());
        assert_eq!(creator, Some("creator".to_string()));

        let version = cw2::get_contract_version(deps.as_ref().storage).unwrap();
        assert_eq!(
            version,
            ContractVersion {
                contract: CONTRACT_NAME.into(),
                version: CONTRACT_VERSION.into(),
            },
        );
    }

    #[test]
    fn proper_owner_initialization() {
        let mut deps = mock_dependencies();

        let info_owner = mock_info("owner", &[]);
        entry::instantiate(
            deps.as_mut(),
            mock_env(),
            info_owner.clone(),
            InstantiateMsg {
                name: "collection_name".into(),
                symbol: "collection_symbol".into(),
                collection_info_extension: None,
                creator: None,
                minter: None,
                withdraw_address: None,
            },
        )
        .unwrap();

        let minter = MINTER.item.load(deps.as_ref().storage).unwrap().owner;
        assert_eq!(minter, Some(info_owner.sender));
        let creator = CREATOR.item.load(deps.as_ref().storage).unwrap().owner;
        assert_eq!(creator, Some(Addr::unchecked("owner")));
    }

    #[test]
    fn test_migrate() {
        let mut deps = mock_dependencies();

        let env = mock_env();
        use cw721_base_016 as v16;
        v16::entry::instantiate(
            deps.as_mut(),
            env.clone(),
            mock_info("owner", &[]),
            v16::InstantiateMsg {
                name: "legacy_name".into(),
                symbol: "legacy_symbol".into(),
                minter: "legacy_minter".into(),
            },
        )
        .unwrap();

        // mint 200 NFTs before migration
        for i in 0..200 {
            let info = mock_info("legacy_minter", &[]);
            let msg = v16::ExecuteMsg::Mint(v16::msg::MintMsg {
                token_id: i.to_string(),
                owner: "owner".into(),
                token_uri: None,
                extension: None,
            });
            v16::entry::execute(deps.as_mut(), env.clone(), info, msg).unwrap();
        }

        // assert new data before migration:
        // - ownership and collection info throws NotFound Error
        MINTER.item.load(deps.as_ref().storage).unwrap_err(); // cw_ownable in v16 is used for minter
        let contract = Cw721Contract::<
            EmptyExtension,
            Empty,
            Empty,
            Empty,
            EmptyCollectionInfoExtension,
        >::default();
        contract.collection_info(deps.as_ref()).unwrap_err();
        // query on minter and creator store also throws NotFound Error
        MINTER.get_ownership(deps.as_ref().storage).unwrap_err();
        CREATOR.get_ownership(deps.as_ref().storage).unwrap_err();
        // - no tokens
        let all_tokens = contract
            .all_tokens(deps.as_ref(), None, Some(MAX_LIMIT))
            .unwrap();
        assert_eq!(all_tokens.tokens.len(), 0);

        // assert legacy data before migration:
        // - version
        let version = cw2::get_contract_version(deps.as_ref().storage)
            .unwrap()
            .version;
        assert_eq!(version, "0.16.0");
        // - legacy minter is set
        let legacy_minter_store: Item<Addr> = Item::new("minter");
        let legacy_minter = legacy_minter_store.load(deps.as_ref().storage).unwrap();
        assert_eq!(legacy_minter, "legacy_minter");
        // - legacy collection info is set
        let legacy_collection_info_store: Item<cw721_016::ContractInfoResponse> =
            Item::new("nft_info");
        let legacy_collection_info = legacy_collection_info_store
            .load(deps.as_ref().storage)
            .unwrap();
        assert_eq!(legacy_collection_info.name, "legacy_name");
        assert_eq!(legacy_collection_info.symbol, "legacy_symbol");
        // - legacy tokens are set
        let indexes = TokenIndexes {
            owner: MultiIndex::new(token_owner_idx, "tokens", "tokens__owner"),
        };
        let legacy_tokens_store: IndexedMap<
            &str,
            NftInfo<EmptyExtension>,
            TokenIndexes<EmptyExtension>,
        > = IndexedMap::new("tokens", indexes);
        let keys = legacy_tokens_store
            .keys(deps.as_ref().storage, None, None, Order::Ascending)
            .collect::<StdResult<Vec<String>>>()
            .unwrap();
        assert_eq!(keys.len(), 200);
        for key in keys {
            let legacy_token = legacy_tokens_store
                .load(deps.as_ref().storage, &key)
                .unwrap();
            assert_eq!(legacy_token.owner.as_str(), "owner");
        }

        entry::migrate(
            deps.as_mut(),
            env.clone(),
            MigrateMsg::WithUpdate {
                minter: None,
                creator: None,
            },
        )
        .unwrap();

        // version
        let version = cw2::get_contract_version(deps.as_ref().storage)
            .unwrap()
            .version;
        assert_eq!(version, CONTRACT_VERSION);
        assert_ne!(version, "0.16.0");

        // assert minter ownership
        let minter_ownership = MINTER
            .get_ownership(deps.as_ref().storage)
            .unwrap()
            .owner
            .map(|a| a.into_string());
        assert_eq!(minter_ownership, Some("legacy_minter".to_string()));

        // assert creator ownership
        let creator_ownership = CREATOR
            .get_ownership(deps.as_ref().storage)
            .unwrap()
            .owner
            .map(|a| a.into_string());
        assert_eq!(creator_ownership, Some("legacy_minter".to_string()));

        // assert collection info
        let collection_info = contract.collection_info(deps.as_ref()).unwrap();
        let legacy_contract_info = CollectionInfo {
            name: "legacy_name".to_string(),
            symbol: "legacy_symbol".to_string(),
            extension: None,
            updated_at: env.block.time,
        };
        assert_eq!(collection_info, legacy_contract_info);

        // assert tokens
        let all_tokens = contract
            .all_tokens(deps.as_ref(), None, Some(MAX_LIMIT))
            .unwrap();
        assert_eq!(all_tokens.tokens.len(), 200);

        // assert legacy data is still there (allowing backward migration in case of issues)
        // - minter
        let legacy_minter = legacy_minter_store.load(deps.as_ref().storage).unwrap();
        assert_eq!(legacy_minter, "legacy_minter");
        // - collection info
        let legacy_collection_info = legacy_collection_info_store
            .load(deps.as_ref().storage)
            .unwrap();
        assert_eq!(legacy_collection_info.name, "legacy_name");
        assert_eq!(legacy_collection_info.symbol, "legacy_symbol");
        // - tokens
        let keys = legacy_tokens_store
            .keys(deps.as_ref().storage, None, None, Order::Ascending)
            .collect::<StdResult<Vec<String>>>()
            .unwrap();
        assert_eq!(keys.len(), 200);
        for key in keys {
            let legacy_token = legacy_tokens_store
                .load(deps.as_ref().storage, &key)
                .unwrap();
            assert_eq!(legacy_token.owner.as_str(), "owner");
        }
    }
}
