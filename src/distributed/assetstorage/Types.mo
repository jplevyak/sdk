module Types {
  public type Contents = Blob;
  public type Path = Text;

  public type BatchId = Nat;
  public type ChunkId = Nat;
  public type Key = Text;
  public type Time = Int;

  public type CreateAssetArguments = {
    key: Key;
    content_type: Text;
  };

  public type SetAssetContentArguments = {
    key: Key;
    content_encoding: Text;
    chunk_ids: [ChunkId]
  };

  public type UnsetAssetContentArguments = {
    key: Key;
    content_encoding: Text;
  };

  public type DeleteAssetArguments = {
    key: Key;
  };

  public type ClearArguments = {
  };

  public type BatchOperationKind = {
    #CreateAsset: CreateAssetArguments;
    #SetAssetContent: SetAssetContentArguments;
    #UnsetAssetContent: UnsetAssetContentArguments;

    #DeleteAsset: DeleteAssetArguments;

    #Clear: ClearArguments;
  };

  public type CommitBatchArguments = {
    batch_id: BatchId;
    operations: [BatchOperationKind];
  };

  public type HeaderField = (Text, Text);

  public type HttpRequest = {
    method: Text;
    url: Text;
    headers: [HeaderField];
    body: Blob;
  };
  public type HttpResponse = {
    status_code: Nat16;
    headers: [HeaderField];
    body: Blob;
  };

  public type HttpGetChunkRequest = {
    url: Text;
    content_encoding: Text;
    index: Nat; // 1+, chunk[0] was returned by http_request
  };
  public type HttpGetChunkResponse = {
    chunk: Blob;
  };
};
