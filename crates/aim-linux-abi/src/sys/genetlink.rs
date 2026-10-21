//! NETLINK_GENERIC's controller (net/netlink/genetlink.c, Linux v6.12):
//! the families the device's kernel registers, of which there is one, the
//! controller itself (`nlctrl`). The device has no Wi-Fi driver, so no
//! `nl80211`; wificond's family lookup finds none, as on a kernel without
//! cfg80211. CTRL_CMD_GETFAMILY answers by name or id and dumps the
//! families; CTRL_CMD_GETPOLICY is not offered, as before v5.10 (#1952).

use super::netlink::{NLM_F_DUMP, NLM_F_MULTI, NLMSG_DONE, attr, attrs, nlmsg};
use crate::errno::{EINVAL, ENOENT, Errno};

/// `GENL_ID_CTRL`.
pub const GENL_ID_CTRL: u16 = 16;
const CTRL_CMD_NEWFAMILY: u8 = 1;
const CTRL_CMD_GETFAMILY: u8 = 3;
const CTRL_ATTR_FAMILY_ID: u16 = 1;
const CTRL_ATTR_FAMILY_NAME: u16 = 2;
const CTRL_ATTR_VERSION: u16 = 3;
const CTRL_ATTR_HDRSIZE: u16 = 4;
const CTRL_ATTR_MAXATTR: u16 = 5;
const CTRL_ATTR_OPS: u16 = 6;
const CTRL_ATTR_MCAST_GROUPS: u16 = 7;
const CTRL_ATTR_OP_ID: u16 = 1;
const CTRL_ATTR_OP_FLAGS: u16 = 2;
const CTRL_ATTR_MCAST_GRP_NAME: u16 = 1;
const CTRL_ATTR_MCAST_GRP_ID: u16 = 2;
const GENL_CMD_CAP_DO: u32 = 0x2;
const GENL_CMD_CAP_DUMP: u32 = 0x4;
const GENL_CMD_CAP_HASPOL: u32 = 0x8;
const NLA_F_NESTED: u16 = 0x8000;
/// The controller's `version` and `maxattr` (CTRL_ATTR_MAX).
const CTRL_VERSION: u32 = 0x2;
const CTRL_MAXATTR: u32 = 10;
/// The controller's one multicast group, "notify", is GENL_ID_CTRL.
const NOTIFY_GROUP: u32 = 16;
const EOPNOTSUPP: Errno = 95;

/// CTRL_CMD_NEWFAMILY describing `nlctrl` (`ctrl_fill_info`).
fn controller(flags: u16, seq: u32, portid: u32) -> Vec<u8> {
    let mut b = vec![CTRL_CMD_NEWFAMILY, CTRL_VERSION as u8, 0, 0];
    attr(&mut b, CTRL_ATTR_FAMILY_NAME, b"nlctrl\0");
    attr(&mut b, CTRL_ATTR_FAMILY_ID, &GENL_ID_CTRL.to_le_bytes());
    attr(&mut b, CTRL_ATTR_VERSION, &CTRL_VERSION.to_le_bytes());
    attr(&mut b, CTRL_ATTR_HDRSIZE, &0u32.to_le_bytes());
    attr(&mut b, CTRL_ATTR_MAXATTR, &CTRL_MAXATTR.to_le_bytes());
    let mut op = Vec::new();
    attr(
        &mut op,
        CTRL_ATTR_OP_ID,
        &(CTRL_CMD_GETFAMILY as u32).to_le_bytes(),
    );
    attr(
        &mut op,
        CTRL_ATTR_OP_FLAGS,
        &(GENL_CMD_CAP_DO | GENL_CMD_CAP_DUMP | GENL_CMD_CAP_HASPOL).to_le_bytes(),
    );
    let mut ops = Vec::new();
    attr(&mut ops, 1 | NLA_F_NESTED, &op);
    attr(&mut b, CTRL_ATTR_OPS | NLA_F_NESTED, &ops);
    let mut group = Vec::new();
    attr(
        &mut group,
        CTRL_ATTR_MCAST_GRP_ID,
        &NOTIFY_GROUP.to_le_bytes(),
    );
    attr(&mut group, CTRL_ATTR_MCAST_GRP_NAME, b"notify\0");
    let mut groups = Vec::new();
    attr(&mut groups, 1 | NLA_F_NESTED, &group);
    attr(&mut b, CTRL_ATTR_MCAST_GROUPS | NLA_F_NESTED, &groups);
    nlmsg(GENL_ID_CTRL, flags, seq, portid, &b)
}

/// Handle one generic netlink request (`genl_rcv_msg`); its replies go to
/// `out`. The errno to report (0: success).
pub fn request(
    ty: u16,
    flags: u16,
    seq: u32,
    portid: u32,
    body: &[u8],
    out: &mut Vec<Vec<u8>>,
) -> Errno {
    if ty != GENL_ID_CTRL {
        return ENOENT;
    }
    // struct genlmsghdr.
    if body.len() < 4 {
        return EINVAL;
    }
    if body[0] != CTRL_CMD_GETFAMILY {
        return EOPNOTSUPP;
    }
    if flags & NLM_F_DUMP == NLM_F_DUMP {
        out.push(controller(NLM_F_MULTI, seq, portid));
        out.push(nlmsg(
            NLMSG_DONE,
            NLM_F_MULTI,
            seq,
            portid,
            &0i32.to_le_bytes(),
        ));
        return 0;
    }
    let a = attrs(body, 4);
    let found = if let Some((_, id)) = a.iter().find(|(t, _)| *t == CTRL_ATTR_FAMILY_ID) {
        id.get(..2) == Some(&GENL_ID_CTRL.to_le_bytes()[..])
    } else if let Some((_, name)) = a.iter().find(|(t, _)| *t == CTRL_ATTR_FAMILY_NAME) {
        name.split(|&c| c == 0).next() == Some(b"nlctrl")
    } else {
        return EINVAL;
    };
    if !found {
        return ENOENT;
    }
    out.push(controller(0, seq, portid));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get_family(attr_ty: u16, value: &[u8]) -> (Errno, Vec<Vec<u8>>) {
        let mut body = vec![CTRL_CMD_GETFAMILY, 1, 0, 0];
        attr(&mut body, attr_ty, value);
        let mut out = Vec::new();
        (request(GENL_ID_CTRL, 1, 9, 77, &body, &mut out), out)
    }

    #[test]
    fn the_controller_is_found_by_name_and_id() {
        let (err, out) = get_family(CTRL_ATTR_FAMILY_NAME, b"nlctrl\0");
        assert_eq!((err, out.len()), (0, 1));
        let m = &out[0];
        assert_eq!(u16::from_le_bytes([m[4], m[5]]), GENL_ID_CTRL);
        assert_eq!(m[16], CTRL_CMD_NEWFAMILY);
        let a = attrs(&m[16..], 4);
        assert!(a.contains(&(CTRL_ATTR_FAMILY_ID, &16u16.to_le_bytes()[..])));
        assert!(a.contains(&(CTRL_ATTR_FAMILY_NAME, &b"nlctrl\0"[..])));
        assert_eq!(get_family(CTRL_ATTR_FAMILY_ID, &16u16.to_le_bytes()).0, 0);
    }

    #[test]
    fn other_families_are_absent() {
        assert_eq!(get_family(CTRL_ATTR_FAMILY_NAME, b"nl80211\0").0, ENOENT);
        assert_eq!(
            get_family(CTRL_ATTR_FAMILY_ID, &28u16.to_le_bytes()).0,
            ENOENT
        );
        let mut out = Vec::new();
        assert_eq!(request(28, 1, 1, 1, &[1, 0, 0, 0], &mut out), ENOENT);
        assert_eq!(
            request(GENL_ID_CTRL, 1, 1, 1, &[10, 1, 0, 0], &mut out),
            EOPNOTSUPP
        );
        assert_eq!(
            request(GENL_ID_CTRL, 1, 1, 1, &[3, 1, 0, 0], &mut out),
            EINVAL
        );
    }

    #[test]
    fn a_dump_lists_the_controller() {
        let mut out = Vec::new();
        assert_eq!(
            request(GENL_ID_CTRL, 0x301, 4, 5, &[3, 1, 0, 0], &mut out),
            0
        );
        assert_eq!(out.len(), 2);
        assert_eq!(u16::from_le_bytes([out[1][4], out[1][5]]), NLMSG_DONE);
    }
}
