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

import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;

import java.util.List;

/**
 * DTO for the config transaction endpoint.
 *
 * <p>Accepts a batch of {@link ConfigMutation} operations to be applied atomically.
 * All mutations succeed or fail as a unit.</p>
 *
 * @param author      the principal authoring this transaction (e.g. username or service identity)
 * @param description optional human-readable description for audit purposes
 * @param mutations   the ordered list of mutations to apply
 */
public record ConfigTransactionDto(
        String author,
        String description,
        List<ConfigMutation> mutations
) {
}
