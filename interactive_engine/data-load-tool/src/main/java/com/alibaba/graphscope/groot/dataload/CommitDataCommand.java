package com.alibaba.graphscope.groot.dataload;

import com.alibaba.graphscope.groot.common.schema.api.GraphEdge;
import com.alibaba.graphscope.groot.common.schema.api.GraphElement;
import com.alibaba.graphscope.groot.dataload.databuild.ColumnMappingInfo;
import com.alibaba.graphscope.groot.sdk.GrootClient;
import com.alibaba.graphscope.proto.groot.DataLoadTargetPb;

import java.io.IOException;
import java.util.HashMap;
import java.util.Map;

public class CommitDataCommand extends DataCommand {
    public CommitDataCommand(String dataPath) throws IOException {
        super(dataPath);
    }

    public void run() {
        GrootClient client =
                GrootClient.newBuilder()
                        .setHosts(graphEndpoint)
                        .setUsername(username)
                        .setPassword(password)
                        .build();
        Map<Long, DataLoadTargetPb> tableToTarget = new HashMap<>();
        for (ColumnMappingInfo columnMappingInfo : columnMappingInfos.values()) {
            DataLoadTargetPb.Builder builder = DataLoadTargetPb.newBuilder();
            GraphElement graphElement = schema.getElement(columnMappingInfo.getLabelId());
            builder.setLabel(graphElement.getLabel());
            if (graphElement instanceof GraphEdge) {
                builder.setSrcLabel(
                        schema.getElement(columnMappingInfo.getSrcLabelId()).getLabel());
                builder.setDstLabel(
                        schema.getElement(columnMappingInfo.getDstLabelId()).getLabel());
            }
            tableToTarget.put(columnMappingInfo.getTableId(), builder.build());
        }
        System.out.println("Commit data. unique path: " + uniquePath);
        client.commitDataLoad(tableToTarget, uniquePath, commitConfig);
        System.out.println("Commit complete.");
    }
}
