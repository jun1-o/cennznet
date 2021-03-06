/* Copyright 2019-2020 Centrality Investments Limited
*
* Licensed under the LGPL, Version 3.0 (the "License");
* you may not use this file except in compliance with the License.
* Unless required by applicable law or agreed to in writing, software
* distributed under the License is distributed on an "AS IS" BASIS,
* WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
* See the License for the specific language governing permissions and
* limitations under the License.
* You may obtain a copy of the License at the root of this project source code,
* or at:
*     https://centrality.ai/licenses/gplv3.txt
*     https://centrality.ai/licenses/lgplv3.txt
*/

use crate::{
	device::{self, DeviceId},
	groups, inbox, response,
};
use frame_support::{decl_error, decl_module, decl_storage, dispatch::Vec, ensure, weights::Weight};
use frame_system::ensure_signed;

mod benchmarking;
mod default_weights;

const MAX_PKBS: usize = 50;
const WITHDRAW_LIST_MAX_LEN: usize = 50;

pub trait WeightInfo {
	fn register_device(p: u32) -> Weight;
	fn replenish_pkbs(p: u32) -> Weight;
	fn withdraw_pkbs(p: u32) -> Weight;
}

pub trait Trait: inbox::Trait + response::Trait + device::Trait + groups::Trait {
	type WeightInfo: WeightInfo;
}

// Serialized pre key bundle used to establish one to one e2ee
pub type PreKeyBundle = Vec<u8>;

decl_error! {
	pub enum Error for Module<T: Trait> {
		/// Cannot store more than MAX_PKBS
		MaxPreKeyBundle,
		/// withdraw list is larger than WITHDRAW_LIST_MAX_LEN elements
		VeryLargeWithdrawList,
	}
}

decl_module! {
	pub struct Module<T: Trait> for enum Call where origin: T::Origin, system = frame_system {

		/// Register a new device for a user
		///
		/// weight:
		/// O(g) where g is the number of groups the user is in
		/// Multiple reads and writes depending on the user states.
		#[weight = <T as Trait>::WeightInfo::register_device(pkbs.len() as u32)]
		fn register_device(origin, device_id: DeviceId, pkbs: Vec<PreKeyBundle>) {
			let sender = ensure_signed(origin)?;

			ensure!(Self::check_total_pkbs(&sender, device_id, pkbs.len()), Error::<T>::MaxPreKeyBundle);

			<device::Module<T>>::append_device(&sender, device_id)?;

			let user_groups = <groups::Memberships<T>>::get(&sender);
			for group_id in user_groups {
				<groups::Module<T>>::append_member_device(&group_id, sender.clone(), device_id);
			}

			<PreKeyBundles<T>>::mutate((sender, device_id), |current_pkbs| current_pkbs.extend(pkbs));
		}

		/// Add a new PreKey bundle for a given user's device.
		///
		/// weight:
		/// O(1)
		/// 1 write.
		#[weight = <T as Trait>::WeightInfo::replenish_pkbs(pkbs.len() as u32)]
		fn replenish_pkbs(origin, device_id: DeviceId, pkbs: Vec<PreKeyBundle>) {
			let sender = ensure_signed(origin)?;

			ensure!(Self::check_total_pkbs(&sender, device_id, pkbs.len()), Error::<T>::MaxPreKeyBundle);

			<PreKeyBundles<T>>::mutate((sender, device_id), |current_pkbs| current_pkbs.extend(pkbs));
		}

		/// Retrieve and remove the Prekey bundles of a given list of user accounts and devices
		///
		/// weight:
		/// O(n * k) where n is the size of input `wanted_pkbs`, and k is the number existing PKBS in the storage
		/// Number of read and write scaled by size of input
		#[weight = <T as Trait>::WeightInfo::withdraw_pkbs(wanted_pkbs.len() as u32)]
		fn withdraw_pkbs(origin, request_id: T::Hash, wanted_pkbs: Vec<(T::AccountId, DeviceId)>) {
			let sender = ensure_signed(origin)?;

			ensure!(wanted_pkbs.len() <= WITHDRAW_LIST_MAX_LEN, Error::<T>::VeryLargeWithdrawList);

			let acquired_pkbs: Vec<(T::AccountId, DeviceId, PreKeyBundle)> = wanted_pkbs
				.into_iter()
				.filter_map(|wanted_pkb| {
					let mut pkbs = <PreKeyBundles<T>>::get(&wanted_pkb);

					pkbs.pop().map(|retrieved_pkb| {
						<PreKeyBundles<T>>::insert(&wanted_pkb, pkbs);
						(wanted_pkb.0, wanted_pkb.1, retrieved_pkb)
					})
				})
				.collect();

			<response::Module<T>>::set_response(sender, request_id, response::Response::PreKeyBundles(acquired_pkbs));
		}
	}
}

decl_storage! {
	trait Store for Module<T: Trait> as SyloE2EE {
		PreKeyBundles get(fn pkbs): map hasher(blake2_128_concat) (T::AccountId, DeviceId) => Vec<PreKeyBundle>;
	}
}

impl<T: Trait> Module<T> {
	fn check_total_pkbs(sender_id: &T::AccountId, device_id: DeviceId, pkbs_count: usize) -> bool {
		let current_pkbs = <PreKeyBundles<T>>::get((sender_id, device_id));
		(current_pkbs.len() + pkbs_count) <= MAX_PKBS
	}
}

#[cfg(test)]
pub(super) mod tests {
	use super::*;
	use crate::mock::{ExtBuilder, Origin, Test};
	use frame_support::assert_ok;
	use sp_core::H256;

	impl Trait for Test {
		type WeightInfo = ();
	}
	impl device::Trait for Test {}
	impl inbox::Trait for Test {
		type WeightInfo = ();
	}
	impl response::Trait for Test {
		type WeightInfo = ();
	}
	impl groups::Trait for Test {
		type WeightInfo = ();
	}
	type E2EE = Module<Test>;
	type Device = device::Module<Test>;
	type Response = response::Module<Test>;

	#[test]
	fn should_add_device() {
		ExtBuilder::default().build().execute_with(|| {
			assert_ok!(E2EE::register_device(Origin::signed(1), 0, vec![]));
			assert_eq!(Device::devices(1).len(), 1);

			assert_ok!(E2EE::register_device(Origin::signed(1), 1, vec![]));
			assert_eq!(Device::devices(1).len(), 2);
			assert_eq!(Device::devices(1)[1], 1);
		});
	}

	#[test]
	fn should_replenish_pkbs() {
		ExtBuilder::default().build().execute_with(|| {
			assert_ok!(E2EE::register_device(Origin::signed(1), 0, vec![]));

			let mock_pkb = b"10".to_vec();

			assert_ok!(E2EE::replenish_pkbs(Origin::signed(1), 0, vec![mock_pkb.clone()]));

			assert_eq!(E2EE::pkbs((1, 0)), vec![mock_pkb]);
		});
	}

	#[test]
	fn should_withdraw_pkbs() {
		ExtBuilder::default().build().execute_with(|| {
			assert_ok!(E2EE::register_device(Origin::signed(1), 0, vec![]));

			let mock_pkb = b"10".to_vec();

			assert_ok!(E2EE::replenish_pkbs(Origin::signed(1), 0, vec![mock_pkb.clone()]));

			let req_id = H256::from([3; 32]);
			let wanted_pkbs = vec![(1, 0)];

			assert_ok!(E2EE::withdraw_pkbs(Origin::signed(2), req_id.clone(), wanted_pkbs));

			assert_eq!(
				Response::response((2, req_id)),
				response::Response::PreKeyBundles(vec![(1, 0, mock_pkb)])
			);
		});
	}
}
