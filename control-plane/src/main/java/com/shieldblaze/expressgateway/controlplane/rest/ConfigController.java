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
package com.shieldblaze.expressgateway.controlplane.rest;

import com.shieldblaze.expressgateway.controlplane.config.ChangeJournal;
import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import com.shieldblaze.expressgateway.controlplane.config.ConfigTransaction;
import com.shieldblaze.expressgateway.controlplane.distribution.ConfigDistributor;
import com.shieldblaze.expressgateway.controlplane.rest.dto.ApiResponse;
import com.shieldblaze.expressgateway.controlplane.rest.dto.ConfigTransactionDto;
import com.shieldblaze.expressgateway.controlplane.rest.dto.ConfigVersionDto;
import lombok.extern.log4j.Log4j2;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

import java.util.ArrayList;
import java.util.List;
import java.util.Map;

/**
 * REST controller for configuration version management and rollback operations.
 *
 * <p>Exposes the change journal as a version history and provides rollback
 * functionality for reverting to previous configuration snapshots.</p>
 */
@RestController
@RequestMapping("/api/v1/controlplane/config")
@Log4j2
public class ConfigController {

    private final ChangeJournal journal;
    private final ConfigDistributor distributor;

    public ConfigController(ChangeJournal journal, ConfigDistributor distributor) {
        this.journal = journal;
        this.distributor = distributor;
    }

    @GetMapping("/versions")
    public ApiResponse<List<ConfigVersionDto>> listVersions() throws Exception {
        List<ChangeJournal.JournalEntry> entries = journal.entriesSince(0);
        List<ConfigVersionDto> versions = entries.stream()
                .map(ConfigVersionDto::from)
                .toList();
        return ApiResponse.ok(versions);
    }

    @GetMapping("/current")
    public ApiResponse<Long> getCurrentVersion() {
        return ApiResponse.ok(journal.currentRevision());
    }

    @PostMapping("/rollback")
    public ApiResponse<String> rollback(@RequestBody Map<String, Long> body) {
        Long targetVersion = body.get("version");
        if (targetVersion == null || targetVersion < 1) {
            throw new IllegalArgumentException("Request body must contain 'version' >= 1");
        }

        long currentVersion = journal.currentRevision();
        if (targetVersion > currentVersion) {
            throw new IllegalArgumentException(
                    "Cannot rollback to version " + targetVersion + " (current version is " + currentVersion + ")");
        }
        if (targetVersion == currentVersion) {
            return ApiResponse.ok("Already at version " + targetVersion, "no-op");
        }

        throw new UnsupportedOperationException(
                "Rollback not yet implemented. Target version: " + targetVersion + ", current: " + currentVersion);
    }

    /**
     * Submit a batch of configuration mutations as an atomic transaction.
     *
     * <p>All mutations are validated before any are applied. If any mutation fails
     * validation, the entire transaction is rejected and no changes are made.
     * On success, all mutations are submitted to the {@link ConfigDistributor}
     * as a single {@link ConfigTransaction}.</p>
     *
     * @param dto the transaction request containing author, description, and mutations
     * @return 200 with the number of mutations applied, 400 if validation fails
     */
    @PostMapping("/transaction")
    public ResponseEntity<ApiResponse<String>> submitTransaction(@RequestBody ConfigTransactionDto dto) {
        // Validate request structure
        if (dto.mutations() == null || dto.mutations().isEmpty()) {
            throw new IllegalArgumentException("Transaction must contain at least one mutation");
        }
        if (dto.author() == null || dto.author().isBlank()) {
            throw new IllegalArgumentException("Transaction author must not be blank");
        }

        // Phase 1: Validate all mutations before applying any (atomic semantics)
        List<String> errors = new ArrayList<>();
        for (int i = 0; i < dto.mutations().size(); i++) {
            ConfigMutation mutation = dto.mutations().get(i);
            if (mutation == null) {
                errors.add("mutation[" + i + "]: must not be null");
                continue;
            }
            if (mutation instanceof ConfigMutation.Upsert upsert) {
                try {
                    ConfigResourceHelper.validateSpec(upsert.resource().kind(), upsert.resource().spec());
                } catch (IllegalArgumentException e) {
                    errors.add("mutation[" + i + "]: " + e.getMessage());
                }
            }
            // Delete mutations are validated by the record's compact constructor (non-null resourceId)
        }

        if (!errors.isEmpty()) {
            throw new IllegalArgumentException(
                    "Transaction validation failed: " + String.join("; ", errors));
        }

        // Phase 2: Build the domain transaction and submit atomically
        ConfigTransaction.Builder builder = ConfigTransaction.builder(dto.author());
        if (dto.description() != null) {
            builder.description(dto.description());
        }
        for (ConfigMutation mutation : dto.mutations()) {
            if (mutation instanceof ConfigMutation.Upsert upsert) {
                builder.upsert(upsert.resource());
            } else if (mutation instanceof ConfigMutation.Delete delete) {
                builder.delete(delete.resourceId());
            }
        }

        ConfigTransaction transaction = builder.build();
        distributor.submit(transaction);

        log.info("Submitted transaction with {} mutations by author '{}'",
                transaction.mutations().size(), transaction.author());

        return ResponseEntity.ok(
                ApiResponse.ok("Transaction accepted",
                        transaction.mutations().size() + " mutations submitted"));
    }
}
