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
package com.shieldblaze.expressgateway.controlplane.config.types;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.shieldblaze.expressgateway.controlplane.config.ConfigSpec;

import java.util.List;
import java.util.Objects;
import java.util.Set;

/**
 * Configuration spec for security policies (ACL, WAF rules).
 *
 * @param name   The security policy name
 * @param type   The policy type ("acl", "waf")
 * @param action The action to take on match ("allow", "deny", "log")
 * @param rules  The list of rule expressions (e.g. CIDR ranges for ACL, signatures for WAF)
 */
public record SecurityPolicySpec(
        @JsonProperty("name") String name,
        @JsonProperty("type") String type,
        @JsonProperty("action") String action,
        @JsonProperty("rules") List<String> rules
) implements ConfigSpec {

    private static final Set<String> VALID_TYPES = Set.of("acl", "waf");
    private static final Set<String> VALID_ACTIONS = Set.of("allow", "deny", "log");

    public SecurityPolicySpec {
        // Defensive copy of rules list to guarantee immutability
        rules = rules == null ? List.of() : List.copyOf(rules);
    }

    @Override
    public void validate() {
        Objects.requireNonNull(name, "name");
        if (name.isBlank()) {
            throw new IllegalArgumentException("name must not be blank");
        }
        Objects.requireNonNull(type, "type");
        if (!VALID_TYPES.contains(type)) {
            throw new IllegalArgumentException("type must be one of " + VALID_TYPES + ", got: " + type);
        }
        Objects.requireNonNull(action, "action");
        if (!VALID_ACTIONS.contains(action)) {
            throw new IllegalArgumentException("action must be one of " + VALID_ACTIONS + ", got: " + action);
        }
        if (rules.isEmpty()) {
            throw new IllegalArgumentException("rules must not be empty");
        }
        for (int i = 0; i < rules.size(); i++) {
            if (rules.get(i) == null || rules.get(i).isBlank()) {
                throw new IllegalArgumentException("rules[" + i + "] must not be null or blank");
            }
        }
    }
}
