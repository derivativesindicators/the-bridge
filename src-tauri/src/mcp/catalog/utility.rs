//! The `utility` toolset: the OpenAlgo version, the order constants and
//! Telegram alerts.

use crate::mcp::schema::{Dflt as D, Param, Ty::*};
use crate::mcp::{Risk, Scope, ToolDef};

pub const TOOLS: &[ToolDef] = &[
    ToolDef {
        name: "get_openalgo_version",
        title: "Get The Bridge Version",
        toolset: "utility",
        scope: Scope::ReadMarket,
        risk: Risk::BrokerStructured,
        write: false,
        destructive: false,
        open_world: false,
        description: r#"Get the The Bridge library version."#,
        params: &[],
    },
    ToolDef {
        name: "validate_order_constants",
        title: "Validate Order Constants",
        toolset: "utility",
        scope: Scope::ReadMarket,
        risk: Risk::BrokerStructured,
        write: false,
        destructive: false,
        open_world: false,
        description: r#"Display all valid order constants for reference."#,
        params: &[],
    },
    ToolDef {
        name: "send_telegram_alert",
        title: "Send Telegram Alert",
        toolset: "utility",
        scope: Scope::ReadAccount,
        risk: Risk::BrokerStructured,
        write: true,
        destructive: false,
        open_world: true,
        description: r#"Send a Telegram alert notification.

    Args:
        username: OpenAlgo login ID/username
        message: Alert message to send
        priority: Notification priority (1-10, default 5). Higher values may be used
                  by the bot for emphasis/sorting depending on configuration.

    Returns:
        JSON with status and message"#,
        params: &[
            Param {
                name: "username",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "message",
                ty: Str,
                default: D::Required,
            },
            Param {
                name: "priority",
                ty: Int,
                default: D::Int(5),
            },
        ],
    },
];
