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

import com.shieldblaze.expressgateway.controlplane.config.ChangeJournal;

/**
 * DTO representing a configuration version (journal entry) in the change history.
 *
 * @param version       the global revision number
 * @param timestamp     ISO-8601 timestamp of when the version was created
 * @param mutationCount the number of mutations in this version
 * @param description   human-readable description (may be null)
 */
public record ConfigVersionDto(
        long version,
        String timestamp,
        int mutationCount,
        String description
) {

    /**
     * Create a DTO from a {@link ChangeJournal.JournalEntry}.
     */
    public static ConfigVersionDto from(ChangeJournal.JournalEntry entry) {
        return new ConfigVersionDto(
                entry.globalRevision(),
                entry.timestamp().toString(),
                entry.mutations().size(),
                null
        );
    }
}
