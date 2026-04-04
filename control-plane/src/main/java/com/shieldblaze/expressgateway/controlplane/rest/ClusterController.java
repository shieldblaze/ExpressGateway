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

import com.shieldblaze.expressgateway.controlplane.cluster.ConfigWriteForwarder;
import com.shieldblaze.expressgateway.controlplane.cluster.ControlPlaneCluster;
import com.shieldblaze.expressgateway.controlplane.config.ConfigKind;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResourceId;
import com.shieldblaze.expressgateway.controlplane.config.types.ClusterSpec;
import com.shieldblaze.expressgateway.controlplane.distribution.ConfigDistributor;
import com.shieldblaze.expressgateway.controlplane.kvstore.KVStore;
import com.shieldblaze.expressgateway.controlplane.rest.dto.ApiResponse;
import com.shieldblaze.expressgateway.controlplane.rest.dto.ClusterDto;
import jakarta.annotation.Nullable;
import lombok.extern.log4j.Log4j2;

import static com.shieldblaze.expressgateway.common.utils.LogSanitizer.sanitize;

import org.springframework.http.HttpStatus;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.DeleteMapping;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.PutMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RequestParam;
import org.springframework.web.bind.annotation.RestController;

import java.util.List;
import java.util.Optional;

/**
 * REST controller for backend cluster configuration management.
 *
 * <p>Provides CRUD operations for cluster configs. Each mutation is persisted
 * to the KV store and submitted to the {@link ConfigDistributor} for propagation
 * to connected data-plane nodes.</p>
 */
@RestController
@RequestMapping("/api/v1/controlplane/clusters")
@Log4j2
public class ClusterController {

    private final KVStore kvStore;
    private final ConfigDistributor distributor;
    private final ControlPlaneCluster cluster;
    private final ConfigWriteForwarder forwarder;

    public ClusterController(KVStore kvStore, ConfigDistributor distributor,
                             @Nullable ControlPlaneCluster cluster,
                             @Nullable ConfigWriteForwarder forwarder) {
        this.kvStore = kvStore;
        this.distributor = distributor;
        this.cluster = cluster;
        this.forwarder = forwarder;
    }

    @PostMapping
    public ApiResponse<String> createCluster(@RequestBody ClusterDto dto,
                                             @RequestParam(defaultValue = "global") String scope) throws Exception {
        ClusterSpec spec = dto.toSpec();
        spec.validate();

        ConfigResource resource = ConfigResourceHelper.createResource(
                ConfigKind.CLUSTER, dto.name(), spec, dto.labels(), scope);
        ConfigResourceHelper.persistAndDistributeWithLeaderCheck(resource, kvStore, distributor, cluster, forwarder);

        log.info("Created cluster: {}", dto.name());
        return ApiResponse.ok("Cluster created", dto.name());
    }

    @GetMapping
    public ApiResponse<List<ClusterDto>> listClusters(
            @RequestParam(defaultValue = "global") String scope) throws Exception {
        List<ConfigResource> resources = ConfigResourceHelper.listResources(ConfigKind.CLUSTER, scope, kvStore);
        List<ClusterDto> dtos = resources.stream()
                .map(r -> ClusterDto.from((ClusterSpec) r.spec(), r.labels()))
                .toList();
        return ApiResponse.ok(dtos);
    }

    @GetMapping("/{clusterId}")
    public ResponseEntity<ApiResponse<ClusterDto>> getCluster(@PathVariable String clusterId,
                                              @RequestParam(defaultValue = "global") String scope) throws Exception {
        ConfigResourceId id = new ConfigResourceId(ConfigKind.CLUSTER.name(), scope, clusterId);
        Optional<ConfigResource> resource = ConfigResourceHelper.getResource(id, kvStore);
        if (resource.isEmpty()) {
            return ResponseEntity.status(HttpStatus.NOT_FOUND)
                    .body(ApiResponse.error("Cluster not found: " + clusterId));
        }
        ConfigResource r = resource.get();
        return ResponseEntity.ok(ApiResponse.ok(ClusterDto.from((ClusterSpec) r.spec(), r.labels())));
    }

    @PutMapping("/{clusterId}")
    public ResponseEntity<ApiResponse<String>> updateCluster(@PathVariable String clusterId, @RequestBody ClusterDto dto,
                                             @RequestParam(defaultValue = "global") String scope) throws Exception {
        ConfigResourceId id = new ConfigResourceId(ConfigKind.CLUSTER.name(), scope, clusterId);
        Optional<ConfigResource> existing = ConfigResourceHelper.getResource(id, kvStore);
        if (existing.isEmpty()) {
            return ResponseEntity.status(HttpStatus.NOT_FOUND)
                    .body(ApiResponse.error("Cluster not found: " + clusterId));
        }

        ClusterSpec spec = dto.toSpec();
        spec.validate();

        ConfigResource updated = ConfigResourceHelper.updateResource(existing.get(), spec, dto.labels());
        ConfigResourceHelper.persistAndDistributeWithLeaderCheck(updated, kvStore, distributor, cluster, forwarder);

        log.info("Updated cluster: {}", sanitize(clusterId));
        return ResponseEntity.ok(ApiResponse.ok("Cluster updated", clusterId));
    }

    @DeleteMapping("/{clusterId}")
    public ApiResponse<String> deleteCluster(@PathVariable String clusterId,
                                             @RequestParam(defaultValue = "global") String scope) throws Exception {
        ConfigResourceId id = new ConfigResourceId(ConfigKind.CLUSTER.name(), scope, clusterId);
        ConfigResourceHelper.deleteAndDistributeWithLeaderCheck(id, kvStore, distributor, cluster, forwarder);

        log.info("Deleted cluster: {}", sanitize(clusterId));
        return ApiResponse.ok("Cluster deleted", clusterId);
    }
}
