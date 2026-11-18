//! The choice of a server's cipher suite and key exchange group.
//!
//! Once it has the ClientHello and the target, a server chooses a cipher
//! suite, which fixes the transcript hash and the AEAD, and a key exchange
//! group. The choice is a [`ParameterChoice`]; [`PreferenceTable`] chooses
//! by fixed preference orders.
//!
//! Whether to send a HelloRetryRequest follows from the chosen group: when
//! the ClientHello has no key share of that group, the server must ask for
//! one (RFC 9846 §4.2.1). The handshake derives this; a choice only names
//! the group.

use crate::{
    alert::AlertDescription,
    cipher_suite::{CipherSuite, SupportedSuite},
    message::client_hello::ClientHello,
    server::targets::SelectedTarget,
};
use nitori_crypto::{Hash, KeyExchange, NamedGroup, Supported};
use smallvec::SmallVec;
use snafu::Snafu;

/// What a ClientHello offers for the choice: its cipher suites, its
/// `supported_groups` and the groups of its `key_share`, as the client
/// sent them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientOffer<'a> {
    cipher_suites: &'a [CipherSuite],
    supported_groups: &'a [NamedGroup],
    key_share_groups: SmallVec<[NamedGroup; 4]>,
}

impl<'a> ClientOffer<'a> {
    /// The offer of `cipher_suites` and `supported_groups`, with key shares
    /// for `key_share_groups`, each in the client's order.
    pub fn new(
        cipher_suites: &'a [CipherSuite],
        supported_groups: &'a [NamedGroup],
        key_share_groups: impl IntoIterator<Item = NamedGroup>,
    ) -> Self {
        Self {
            cipher_suites,
            supported_groups,
            key_share_groups: key_share_groups.into_iter().collect(),
        }
    }

    /// The offer of `hello`; a missing `supported_groups` or `key_share`
    /// offers no groups or no shares.
    pub fn from_client_hello<C>(hello: &'a ClientHello<C>) -> Self {
        let supported_groups = hello
            .supported_groups()
            .map_or(&[][..], |groups| groups.as_slice());
        let key_share_groups = hello
            .key_share()
            .into_iter()
            .flat_map(|shares| shares.entries())
            .map(|entry| entry.group());
        Self::new(
            hello.cipher_suites().as_slice(),
            supported_groups,
            key_share_groups,
        )
    }

    /// The cipher suites, in the client's order.
    pub fn cipher_suites(&self) -> &'a [CipherSuite] {
        self.cipher_suites
    }

    /// The groups of `supported_groups`, in the client's order.
    pub fn supported_groups(&self) -> &'a [NamedGroup] {
        self.supported_groups
    }

    /// The groups the client sent a key share for, in its order.
    pub fn key_share_groups(&self) -> &[NamedGroup] {
        &self.key_share_groups
    }

    /// Whether the client sent a key share of `group`.
    pub fn has_key_share(&self, group: NamedGroup) -> bool {
        self.key_share_groups.contains(&group)
    }
}

/// A server's choice: a cipher suite and a key exchange group, both
/// supported by the backend `C`.
pub struct Choice<C> {
    suite: SupportedSuite<C>,
    group: Supported<C, NamedGroup>,
}

impl<C> Choice<C> {
    /// The choice of `suite` and `group`.
    pub fn new(suite: SupportedSuite<C>, group: Supported<C, NamedGroup>) -> Self {
        Self { suite, group }
    }

    /// The cipher suite.
    pub fn suite(&self) -> SupportedSuite<C> {
        self.suite
    }

    /// The key exchange group.
    pub fn group(&self) -> Supported<C, NamedGroup> {
        self.group
    }
}

impl<C> Clone for Choice<C> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<C> Copy for Choice<C> {}

impl<C> core::fmt::Debug for Choice<C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Choice")
            .field("suite", &self.suite)
            .field("group", &self.group)
            .finish()
    }
}

/// The ClientHello and the server have no parameter in common.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Snafu)]
pub enum NoCommonParametersError {
    /// No cipher suite the client offers is one the server accepts and
    /// the backend supports.
    #[snafu(display("no common cipher suite"))]
    NoCommonSuite,
    /// No group of the client's `supported_groups` is one the server
    /// accepts and the backend supports.
    #[snafu(display("no common key exchange group"))]
    NoCommonGroup,
}

impl NoCommonParametersError {
    /// The alert the failure calls for: handshake_failure, one of the two
    /// RFC 9846 §4.2.1 allows.
    pub fn alert(&self) -> AlertDescription {
        AlertDescription::HANDSHAKE_FAILURE
    }
}

/// How a server chooses its cipher suite and key exchange group for a
/// ClientHello.
///
/// `T` is the type of the target the server selected for the ClientHello.
/// The choice has no state per connection; state across connections, such
/// as load, belongs to the implementation.
pub trait ParameterChoice<C, T> {
    /// The suite and group for `offer`, on `backend`, for `selected`.
    ///
    /// # Errors
    ///
    /// [`NoCommonParametersError`] when no suite or no group fits.
    fn choose(
        &self,
        backend: &C,
        offer: &ClientOffer<'_>,
        selected: &SelectedTarget<T>,
    ) -> Result<Choice<C>, NoCommonParametersError>;
}

/// Which group a [`PreferenceTable`] takes among those both sides accept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GroupPreference {
    /// The first group in the table's order that the client sent a key
    /// share for, so that no HelloRetryRequest is needed; without such a
    /// group, the first in the table's order.
    KeyShareFirst,
    /// The first group in the table's order, with a HelloRetryRequest when
    /// the client sent no key share for it.
    TableOrder,
}

/// A choice by fixed preference orders: the first suite in the table's
/// order that the client offers and the backend supports, and a group by
/// the table's order and its [`GroupPreference`].
///
/// The client's own order does not count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreferenceTable {
    suites: Vec<CipherSuite>,
    groups: Vec<NamedGroup>,
    group_preference: GroupPreference,
}

impl PreferenceTable {
    /// The table of `suites` and `groups`, each most preferred first.
    pub fn new(
        suites: Vec<CipherSuite>,
        groups: Vec<NamedGroup>,
        group_preference: GroupPreference,
    ) -> Self {
        Self {
            suites,
            groups,
            group_preference,
        }
    }

    /// The suites, most preferred first.
    pub fn suites(&self) -> &[CipherSuite] {
        &self.suites
    }

    /// The groups, most preferred first.
    pub fn groups(&self) -> &[NamedGroup] {
        &self.groups
    }

    /// How the table takes a group.
    pub fn group_preference(&self) -> GroupPreference {
        self.group_preference
    }
}

impl<C: Hash + KeyExchange, T> ParameterChoice<C, T> for PreferenceTable {
    fn choose(
        &self,
        backend: &C,
        offer: &ClientOffer<'_>,
        _selected: &SelectedTarget<T>,
    ) -> Result<Choice<C>, NoCommonParametersError> {
        let suite = self
            .suites
            .iter()
            .filter(|suite| offer.cipher_suites().contains(suite))
            .find_map(|&suite| SupportedSuite::new(backend, suite))
            .ok_or(NoCommonParametersError::NoCommonSuite)?;
        let mut common = self
            .groups
            .iter()
            .filter(|group| offer.supported_groups().contains(group))
            .filter_map(|&group| backend.named_group(group));
        let group = match self.group_preference {
            GroupPreference::TableOrder => common.next(),
            GroupPreference::KeyShareFirst => {
                let common: SmallVec<[Supported<C, NamedGroup>; 8]> = common.collect();
                common
                    .iter()
                    .find(|group| offer.has_key_share(group.algorithm()))
                    .or(common.first())
                    .copied()
            }
        }
        .ok_or(NoCommonParametersError::NoCommonGroup)?;
        Ok(Choice::new(suite, group))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClientOffer, GroupPreference, NoCommonParametersError, ParameterChoice, PreferenceTable,
    };
    use crate::{
        cipher_suite::CipherSuite,
        message::client_hello::codec::read_client_hello,
        server::targets::SelectedTarget,
        test_support::{client_hello, concat, extension, vector},
    };
    use nitori_crypto::NamedGroup;
    use nitori_crypto_aws_lc::AwsLcBackend;

    const SUITES: &[CipherSuite] = &[
        CipherSuite::TLS_CHACHA20_POLY1305_SHA256,
        CipherSuite::TLS_AES_128_GCM_SHA256,
    ];
    const GROUPS: &[NamedGroup] = &[NamedGroup::SECP256R1, NamedGroup::X25519];

    fn table(group_preference: GroupPreference) -> PreferenceTable {
        PreferenceTable::new(
            vec![
                CipherSuite::TLS_AES_128_CCM_SHA256,
                CipherSuite::TLS_AES_128_GCM_SHA256,
                CipherSuite::TLS_CHACHA20_POLY1305_SHA256,
            ],
            vec![
                NamedGroup::X25519MLKEM768,
                NamedGroup::X25519,
                NamedGroup::SECP256R1,
            ],
            group_preference,
        )
    }

    fn selected() -> SelectedTarget<()> {
        SelectedTarget {
            target: (),
            application_protocol: None,
        }
    }

    #[test]
    fn the_table_order_wins_over_the_clients() {
        let offer = ClientOffer::new(SUITES, GROUPS, [NamedGroup::SECP256R1]);
        let choice = table(GroupPreference::TableOrder)
            .choose(&AwsLcBackend, &offer, &selected())
            .unwrap();
        assert_eq!(choice.suite().suite(), CipherSuite::TLS_AES_128_GCM_SHA256);
        // x25519 comes first in the table, though the client shared only
        // secp256r1: the handshake asks for an x25519 share.
        assert_eq!(choice.group().algorithm(), NamedGroup::X25519);
    }

    #[test]
    fn a_shared_group_avoids_a_retry() {
        let offer = ClientOffer::new(SUITES, GROUPS, [NamedGroup::SECP256R1]);
        let choice = table(GroupPreference::KeyShareFirst)
            .choose(&AwsLcBackend, &offer, &selected())
            .unwrap();
        assert_eq!(choice.group().algorithm(), NamedGroup::SECP256R1);
        let unshared = ClientOffer::new(SUITES, GROUPS, []);
        let choice = table(GroupPreference::KeyShareFirst)
            .choose(&AwsLcBackend, &unshared, &selected())
            .unwrap();
        assert_eq!(choice.group().algorithm(), NamedGroup::X25519);
    }

    #[test]
    fn nothing_in_common_fails() {
        let table = table(GroupPreference::TableOrder);
        let no_suite = ClientOffer::new(
            &[CipherSuite::TLS_AES_256_GCM_SHA384],
            GROUPS,
            [NamedGroup::X25519],
        );
        assert_eq!(
            table
                .choose(&AwsLcBackend, &no_suite, &selected())
                .unwrap_err(),
            NoCommonParametersError::NoCommonSuite
        );
        let no_group = ClientOffer::new(SUITES, &[NamedGroup::SECP384R1], []);
        let error = table
            .choose(&AwsLcBackend, &no_group, &selected())
            .unwrap_err();
        assert_eq!(error, NoCommonParametersError::NoCommonGroup);
        assert_eq!(
            error.alert(),
            crate::alert::AlertDescription::HANDSHAKE_FAILURE
        );
    }

    #[test]
    fn the_offer_of_a_client_hello() {
        let groups = vector(2, &[0x00, 0x1d, 0x00, 0x17]);
        let shares = vector(2, &concat(&[&[0x00, 0x17], &vector(2, &[4; 65])]));
        let bytes = client_hello(&concat(&[
            &extension(10, &groups),
            &extension(43, crate::test_support::VERSIONS),
            &extension(51, &shares),
        ]));
        let hello = read_client_hello(&mut &bytes[..]).unwrap();
        let offer = ClientOffer::from_client_hello(&hello);
        assert_eq!(
            offer.cipher_suites(),
            &[CipherSuite::TLS_AES_128_GCM_SHA256]
        );
        assert_eq!(
            offer.supported_groups(),
            &[NamedGroup::X25519, NamedGroup::SECP256R1]
        );
        assert_eq!(offer.key_share_groups(), &[NamedGroup::SECP256R1]);
        assert!(!offer.has_key_share(NamedGroup::X25519));
    }
}
