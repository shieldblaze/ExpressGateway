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
package com.shieldblaze.expressgateway.controlplane.rest.dto;

import com.shieldblaze.expressgateway.controlplane.config.types.RoutingRuleSpec;

/**
 * DTO for routing rule CRUD operations.
 *
 * @param name          the rule name
 * @param priority      rule evaluation priority (lower = higher priority)
 * @param matchType     the match criteria type ("host", "path", "header")
 * @param matchValue    the value to match against
 * @param targetCluster reference to the target cluster by name
 */
public record RouteDto(
        String name,
        int priority,
        String matchType,
        String matchValue,
        String targetCluster
) {

    /**
     * Convert this DTO to a {@link RoutingRuleSpec}.
     */
    public RoutingRuleSpec toSpec() {
        return new RoutingRuleSpec(name, priority, matchType, matchValue, targetCluster);
    }

    /**
     * Create a DTO from a {@link RoutingRuleSpec}.
     */
    public static RouteDto from(RoutingRuleSpec spec) {
        return new RouteDto(
                spec.name(),
                spec.priority(),
                spec.matchType(),
                spec.matchValue(),
                spec.targetCluster()
        );
    }
}
