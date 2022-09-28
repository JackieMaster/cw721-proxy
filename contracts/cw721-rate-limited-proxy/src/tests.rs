use cosmwasm_std::{to_binary, Addr, Empty};
use cw721_base::MintMsg;
use cw_multi_test::{next_block, App, Contract, ContractWrapper, Executor};
use cw_rate_limiter::Rate;

use crate::msg::InstantiateMsg;

struct Test {
    pub app: App,
    pub cw721s: Vec<Addr>,
    pub minter: Addr,
    pub rate_limiter: Addr,
    pub mock_receiver: Addr,
    nfts_minted: usize,
}

impl Test {
    pub fn new(cw721s: usize, rate: Rate) -> Self {
        let mut app = App::default();
        let minter = Addr::unchecked("minter");

        let cw721_id = app.store_code(cw721_base());
        let rate_limiter_id = app.store_code(cw721_rate_limiter());
        let proxy_tester_id = app.store_code(cw721_proxy_tester());

        let mock_receiver = app
            .instantiate_contract(
                proxy_tester_id,
                minter.clone(),
                &cw721_proxy_tester::msg::InstantiateMsg::default(),
                &[],
                "proxy_tester",
                None,
            )
            .unwrap();

        let rate_limiter = app
            .instantiate_contract(
                rate_limiter_id,
                minter.clone(),
                &InstantiateMsg::new(rate, Some(mock_receiver.to_string())),
                &[],
                "rate_limiter",
                None,
            )
            .unwrap();

        let cw721_instantiate_msg = |id: usize| cw721_base::msg::InstantiateMsg {
            name: format!("cw721 {}", id),
            symbol: format!("{}", id),
            minter: minter.to_string(),
        };
        let cw721s: Vec<_> = (0..cw721s)
            .map(|id| {
                app.instantiate_contract(
                    cw721_id,
                    minter.clone(),
                    &cw721_instantiate_msg(id),
                    &[],
                    format!("cw721 {}", id),
                    None,
                )
                .unwrap()
            })
            .collect();

        Self {
            app,
            cw721s,
            minter,
            rate_limiter,
            mock_receiver,
            nfts_minted: 0,
        }
    }

    pub fn send_nft_and_check_received(&mut self, nft: Addr) -> Result<(), anyhow::Error> {
        self.nfts_minted += 1;

        self.app.execute_contract(
            self.minter.clone(),
            nft.clone(),
            &cw721_base::msg::ExecuteMsg::<Empty, Empty>::Mint(MintMsg::<Empty> {
                token_id: self.nfts_minted.to_string(),
                owner: self.minter.to_string(),
                token_uri: None,
                extension: Default::default(),
            }),
            &[],
        )?;
        self.app.execute_contract(
            self.minter.clone(),
            nft.clone(),
            &cw721_base::msg::ExecuteMsg::<Empty, Empty>::SendNft {
                contract: self.rate_limiter.to_string(),
                token_id: self.nfts_minted.to_string(),
                msg: to_binary("hello")?,
            },
            &[],
        )?;

        let msg: cw721_proxy_tester::msg::ExecuteMsg = self.app.wrap().query_wasm_smart(
            &self.mock_receiver,
            &cw721_proxy_tester::msg::QueryMsg::LastMsg {},
        )?;

        match msg {
            cw721_proxy_tester::msg::ExecuteMsg::ReceiveProxyNft { eyeball, msg } => {
                assert_eq!(eyeball, nft);
                assert_eq!(
                    msg,
                    cw721::Cw721ReceiveMsg {
                        sender: self.minter.to_string(),
                        token_id: self.nfts_minted.to_string(),
                        msg: to_binary("hello")?
                    }
                )
            }
        }

        Ok(())
    }

    pub fn send_nfts_at_rate<R: rand::Rng>(
        &mut self,
        rng: &mut R,
        rate: Rate,
        for_blocks: usize,
    ) -> Result<(), anyhow::Error> {
        use rand::seq::SliceRandom;

        let start_block = self.app.block_info().height;
        for _ in 0..for_blocks {
            match rate {
                Rate::PerBlock(n) => {
                    for _ in 0..n {
                        let nft = self.cw721s.choose(rng).unwrap().clone();
                        self.send_nft_and_check_received(nft)?;
                    }
                }
                Rate::Blocks(b) => {
                    if (self.app.block_info().height - start_block) % b == 0 {
                        let nft = self.cw721s.choose(rng).unwrap().clone();
                        self.send_nft_and_check_received(nft)?;
                    }
                }
            }
            self.app.update_block(next_block)
        }

        Ok(())
    }
}

impl InstantiateMsg {
    fn new(rate_limit: Rate, origin: Option<String>) -> Self {
        Self { rate_limit, origin }
    }
}

fn cw721_rate_limiter() -> Box<dyn Contract<Empty>> {
    let contract = ContractWrapper::new(
        crate::contract::execute,
        crate::contract::instantiate,
        crate::contract::query,
    );
    Box::new(contract)
}

fn cw721_base() -> Box<dyn Contract<Empty>> {
    let contract = ContractWrapper::new(
        cw721_base::entry::execute,
        cw721_base::entry::instantiate,
        cw721_base::entry::query,
    );
    Box::new(contract)
}

fn cw721_proxy_tester() -> Box<dyn Contract<Empty>> {
    let contract = ContractWrapper::new(
        cw721_proxy_tester::contract::execute,
        cw721_proxy_tester::contract::instantiate,
        cw721_proxy_tester::contract::query,
    );
    Box::new(contract)
}

// Generates a random rate with an internal value within RANGE.
fn random_rate<R: rand::Rng, S: rand::distributions::uniform::SampleRange<u64>>(
    rng: &mut R,
    range: S,
) -> Rate {
    let t = rng.gen();
    let v = rng.gen_range(range);
    match t {
        true => Rate::Blocks(v),
        false => Rate::PerBlock(v),
    }
}

#[test]
fn simple_send() {
    let mut test = Test::new(1, Rate::Blocks(1));
    test.send_nft_and_check_received(test.cw721s[0].clone())
        .unwrap()
}

#[test]
fn test_simple() {
    let rng = &mut rand::thread_rng();
    let expected = Rate::PerBlock(2);
    let actual = Rate::Blocks(1);
    let mut test = Test::new(10, expected);
    test.send_nfts_at_rate(rng, actual, 1).unwrap();
}

#[test]
fn fuzz_rate_limiting() {
    let iterations = 500;
    let max = 5;
    let range = 1..max;
    let rng = &mut rand::thread_rng();

    let limit = random_rate(rng, range.clone());
    let mut test = Test::new(max as usize, limit);

    for _ in 0..iterations {
        let actual = random_rate(rng, range.clone());
        let res = test.send_nfts_at_rate(rng, actual, max as usize);
        let pass = match actual > limit {
            true => res.is_err(),
            false => res.is_ok(),
        };
        if !pass {
            panic!(
                "test failed on (limit, actual) = ({:?}, {:?})",
                limit, actual
            )
        }

        // Open state for next iteration.
        if let Rate::Blocks(blocks) = limit {
            test.app.update_block(|mut b| b.height += blocks);
        } else {
            test.app.update_block(next_block)
        }
    }
}