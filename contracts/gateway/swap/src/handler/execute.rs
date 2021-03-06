// Modifications copyright (C) 2021 Terraformer Corp.
use cosmwasm_bignumber::Uint256;
use cosmwasm_std::*;
use cw20::Cw20ExecuteMsg;
use pylon_utils::tax::deduct_tax;

use crate::error::ContractError;
use crate::querier::strategy;
use crate::state::{config, state, user};

pub fn deposit(deps: DepsMut, env: Env, info: MessageInfo) -> Result<Response, ContractError> {
    let config = config::read(deps.storage).load()?;
    let state = state::read(deps.storage).load()?;

    if env.block.time.seconds() < config.start {
        return Err(ContractError::SwapNotStarted {
            start: config.start,
        });
    }
    if config.finish < env.block.time.seconds() {
        return Err(ContractError::SwapFinished {
            finish: config.finish,
        });
    }

    // 1:1
    let swapped_in: Uint256 = info
        .funds
        .iter()
        .find(|c| c.denom == state.x_denom)
        .map(|c| Uint256::from(c.amount))
        .unwrap_or_else(Uint256::zero);
    if swapped_in.is_zero() {
        return Err(ContractError::NotAllowZeroAmount {});
    }
    if info.funds.len() > 1 {
        return Err(ContractError::NotAllowOtherDenoms {
            denom: state.x_denom,
        });
    }

    let sender = &deps.api.addr_canonicalize(info.sender.as_str())?;
    let mut user = user::read(deps.storage, sender)?;
    let mut state = state::read(deps.storage).load()?;

    // check whitelisted, or free to participate everyone
    if config.whitelist_enabled && !user.whitelisted {
        return Err(ContractError::NotAllowNonWhitelisted {
            address: info.sender.to_string(),
        });
    }

    // check available cap via calling cap_strategy contract
    if let Some(strategy) = config.cap_strategy {
        let available_cap =
            strategy::available_cap_of(deps.as_ref(), strategy, info.sender.to_string())?;

        if available_cap < swapped_in {
            return Err(ContractError::AvailableCapExceeded {
                available: available_cap,
            });
        }
    } // or remains cap strategy to unlimited

    // check swap pool size
    let swapped_out = swapped_in / config.price;
    if state.total_swapped + swapped_out > config.swap_pool_size {
        return Err(ContractError::PoolSizeExceeded {
            available: config.swap_pool_size - state.total_swapped,
        });
    }

    user.swapped_in += swapped_in;
    user.swapped_out += swapped_out;
    state.total_swapped += swapped_out;

    user::store(deps.storage, sender, &user)?;
    state::store(deps.storage).save(&state)?;

    Ok(Response::new()
        .add_attribute("action", "deposit")
        .add_attribute("sender", info.sender.to_string())
        .add_attribute("swapped_in", swapped_in.to_string())
        .add_attribute("swapped_out", swapped_out.to_string()))
}

pub fn claim(deps: DepsMut, env: Env, info: MessageInfo) -> Result<Response, ContractError> {
    let sender = &deps.api.addr_canonicalize(info.sender.as_str()).unwrap();
    let mut user = user::read(deps.storage, sender).unwrap();
    let mut state = state::read(deps.storage).load().unwrap();

    let claimable_token = strategy::claimable_token_of(
        deps.as_ref(),
        env.block.time.seconds(),
        info.sender.to_string(),
    )?;

    user.swapped_out_claimed += claimable_token;
    state.total_claimed += claimable_token;

    user::store(deps.storage, sender, &user)?;
    state::store(deps.storage).save(&state)?;

    Ok(Response::new()
        .add_message(CosmosMsg::Wasm(WasmMsg::Execute {
            contract_addr: state.y_addr,
            msg: to_binary(&Cw20ExecuteMsg::Transfer {
                recipient: info.sender.to_string(),
                amount: claimable_token.into(),
            })?,
            funds: vec![],
        }))
        .add_attribute("action", "claim")
        .add_attribute("sender", info.sender.to_string())
        .add_attribute("amount", claimable_token.to_string()))
}

pub fn earn(deps: DepsMut, _env: Env, info: MessageInfo, amount: Uint256) -> Result<Response, ContractError> {
    let config = config::read(deps.storage).load().unwrap();
    let state = state::read(deps.storage).load().unwrap();
    if config.beneficiary != info.sender {
        return Err(ContractError::Unauthorized {
            action: "earn".to_string(),
            expected: config.beneficiary,
            actual: info.sender.to_string(),
        });
    }

    let earn_amount = amount;
    Ok(Response::new()
        .add_message(CosmosMsg::Bank(BankMsg::Send {
            to_address: config.beneficiary,
            amount: vec![deduct_tax(
                deps.as_ref(),
                Coin {
                    denom: state.x_denom,
                    amount: earn_amount.into(),
                },
            )?],
        }))
        .add_attribute("action", "earn")
        .add_attribute("sender", info.sender.to_string())
        .add_attribute("amount", earn_amount.to_string()))
}
