/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
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
package com.shieldblaze.expressgateway.controlplane.kvstore;

/**
 * Represents a watch event emitted when a key or prefix is modified.
 *
 * @param type          The type of mutation (PUT or DELETE)
 * @param entry         The current entry after the event (null for DELETE if data is unavailable)
 * @param previousEntry The previous entry before the event (null if not available or for initial creates)
 */
public record KVWatchEvent(Type type, KVEntry entry, KVEntry previousEntry) {

    public enum Type {
        PUT,
        DELETE
    }
}
