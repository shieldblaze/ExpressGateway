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
package com.shieldblaze.expressgateway.coordination;

import java.util.Objects;

/**
 * Event emitted by a key/prefix watch.
 *
 * <p>For {@link Type#PUT} events, {@code entry} is the new/updated entry and
 * {@code previousEntry} is the prior state (null if this is a creation).
 * For {@link Type#DELETE} events, {@code entry} is null and
 * {@code previousEntry} is the deleted entry.</p>
 *
 * @param type          the mutation type
 * @param entry         the current entry (null for DELETE events)
 * @param previousEntry the previous entry (null for initial creation PUT events)
 */
public record WatchEvent(Type type, CoordinationEntry entry, CoordinationEntry previousEntry) {

    /**
     * The type of mutation observed.
     */
    public enum Type {
        /** A key was created or updated. */
        PUT,
        /** A key was deleted. */
        DELETE
    }

    public WatchEvent {
        Objects.requireNonNull(type, "type");
    }
}
