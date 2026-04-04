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
package com.shieldblaze.expressgateway.controlplane.distribution;

import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;

import java.util.List;
import java.util.Objects;

/**
 * Delta between two config versions, ready for distribution to nodes.
 *
 * <p>A delta captures the set of mutations required to advance a node from
 * {@code fromRevision} to {@code toRevision}. When distributed to a node,
 * the node applies the mutations in order and then ACKs with {@code toRevision}
 * as its new applied config version.</p>
 *
 * @param fromRevision the exclusive lower bound revision (the node's current version)
 * @param toRevision   the inclusive upper bound revision (the target version after applying)
 * @param mutations    the ordered list of mutations to apply (defensively copied to guarantee immutability)
 */
public record ConfigDelta(
        long fromRevision,
        long toRevision,
        List<ConfigMutation> mutations
) {

    public ConfigDelta {
        Objects.requireNonNull(mutations, "mutations");
        mutations = List.copyOf(mutations);
    }

    /**
     * Returns {@code true} if this delta contains no mutations.
     * An empty delta means the node is already at the target revision.
     */
    public boolean isEmpty() {
        return mutations.isEmpty();
    }
}
