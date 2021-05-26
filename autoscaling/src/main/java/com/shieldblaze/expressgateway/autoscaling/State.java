/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.autoscaling;

public enum State {

    /**
     * The server is under normal load and functioning properly.
     */
    NORMAL,

    /**
     * If a server has reached scale out load (default 75%),
     * a new server will be created by Autoscaling
     * to distribute the load.
     */
    COOLDOWN,

    /**
     * If a server has reached hibernate load (default 90%),
     * DNS record pointing to this server will be removed
     * so that new traffic can no further reach this server
     * and autoscaling will also create new server to distribute
     * the load.
     */
    HIBERNATE
}
