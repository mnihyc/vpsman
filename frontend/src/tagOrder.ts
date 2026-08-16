export type TagOrderBlock = {
  id: string;
  names: string[];
  namespace: string | null;
};

export function tagNamespace(name: string): string | null {
  const separator = name.indexOf(":");
  if (separator <= 0) {
    return null;
  }
  return asciiLower(name.slice(0, separator));
}

export function tagNamespaceDisplayLabel(name: string): string {
  const separator = name.indexOf(":");
  return separator > 0 ? name.slice(0, separator + 1) : name;
}

export function buildTagOrderBlocks(names: readonly string[]): TagOrderBlock[] {
  const blocks: TagOrderBlock[] = [];
  for (const name of names) {
    const namespace = tagNamespace(name);
    const previous = blocks[blocks.length - 1];
    if (namespace !== null && previous?.namespace === namespace) {
      previous.names.push(name);
      previous.id = tagOrderBlockId(namespace, previous.names);
      continue;
    }
    blocks.push({
      id:
        namespace === null
          ? tagOrderLeafId(name)
          : tagOrderBlockId(namespace, [name]),
      names: [name],
      namespace,
    });
  }
  return blocks;
}

export function tagOrderBlockId(
  namespace: string,
  names: readonly string[],
): string {
  const stableNames = names.slice().sort(compareExactAscii);
  return `tag-block:${encodeURIComponent(namespace)}:${stableNames
    .map((name) => encodeURIComponent(name))
    .join("|")}`;
}

export function tagOrderLeafId(name: string): string {
  return `tag-leaf:${encodeURIComponent(name)}`;
}

export function flattenTagOrderBlocks(
  blocks: readonly TagOrderBlock[],
): string[] {
  return blocks.flatMap((block) => block.names);
}

export function compareTagNamesNaturally(left: string, right: string): number {
  const leftSeparator = left.indexOf(":");
  const rightSeparator = right.indexOf(":");
  const leftValue = leftSeparator >= 0 ? left.slice(leftSeparator + 1) : left;
  const rightValue =
    rightSeparator >= 0 ? right.slice(rightSeparator + 1) : right;
  return (
    compareNaturalAscii(leftValue, rightValue) || compareExactAscii(left, right)
  );
}

export function naturallySortedTagNames(names: readonly string[]): string[] {
  return names.slice().sort(compareTagNamesNaturally);
}

export function naturallySortTagOrderBlock(
  orderedNames: readonly string[],
  blockId: string,
): string[] {
  return flattenTagOrderBlocks(
    buildTagOrderBlocks(orderedNames).map((block) =>
      block.id === blockId && block.namespace !== null
        ? { ...block, names: naturallySortedTagNames(block.names) }
        : block,
    ),
  );
}

export function normalizeNaturalTagOrder(
  orderedNames: readonly string[],
): string[] {
  return flattenTagOrderBlocks(
    buildTagOrderBlocks(orderedNames).map((block) =>
      block.namespace === null
        ? block
        : { ...block, names: naturallySortedTagNames(block.names) },
    ),
  );
}

export function moveTagOrderBlock(
  orderedNames: readonly string[],
  activeBlockId: string,
  overBlockId: string,
  naturalSortEnabled: boolean,
): string[] {
  const blocks = buildTagOrderBlocks(orderedNames);
  const activeIndex = blocks.findIndex((block) => block.id === activeBlockId);
  const overIndex = blocks.findIndex((block) => block.id === overBlockId);
  if (activeIndex < 0 || overIndex < 0 || activeIndex === overIndex) {
    return orderedNames.slice();
  }
  const nextBlocks = arrayMove(blocks, activeIndex, overIndex);
  const nextNames = flattenTagOrderBlocks(nextBlocks);
  return naturalSortEnabled ? normalizeNaturalTagOrder(nextNames) : nextNames;
}

export function moveTagOrderLeaf(
  orderedNames: readonly string[],
  activeName: string,
  overName: string,
  naturalSortEnabled: boolean,
): string[] {
  const activeIndex = orderedNames.indexOf(activeName);
  const overIndex = orderedNames.indexOf(overName);
  if (activeIndex < 0 || overIndex < 0 || activeIndex === overIndex) {
    return orderedNames.slice();
  }
  const nextNames = arrayMove(orderedNames, activeIndex, overIndex);
  return naturalSortEnabled ? normalizeNaturalTagOrder(nextNames) : nextNames;
}

export function insertNewTagIntoOrder(
  orderedNames: readonly string[],
  name: string,
  naturalSortEnabled: boolean,
): string[] {
  if (orderedNames.includes(name)) {
    return orderedNames.slice();
  }
  const namespace = tagNamespace(name);
  let insertAt = orderedNames.length;
  if (namespace !== null) {
    const blocks = buildTagOrderBlocks(orderedNames);
    const matchingBlock = blocks
      .slice()
      .reverse()
      .find((block) => block.namespace === namespace);
    if (matchingBlock) {
      const lastName = matchingBlock.names[matchingBlock.names.length - 1];
      if (lastName) {
        insertAt = orderedNames.indexOf(lastName) + 1;
      }
    }
  }
  const nextNames = orderedNames.slice();
  nextNames.splice(insertAt, 0, name);
  return naturalSortEnabled ? normalizeNaturalTagOrder(nextNames) : nextNames;
}

export function reconcileTagOrderDraft(
  draftNames: readonly string[],
  incomingNames: readonly string[],
  naturalSortEnabled: boolean,
): string[] {
  const incomingSet = new Set(incomingNames);
  let reconciled = draftNames.filter((name) => incomingSet.has(name));
  for (const name of incomingNames) {
    if (!reconciled.includes(name)) {
      reconciled = insertNewTagIntoOrder(reconciled, name, naturalSortEnabled);
    }
  }
  return naturalSortEnabled ? normalizeNaturalTagOrder(reconciled) : reconciled;
}

export function sameTagOrder(
  left: readonly string[],
  right: readonly string[],
): boolean {
  return (
    left.length === right.length &&
    left.every((name, index) => name === right[index])
  );
}

function compareNaturalAscii(left: string, right: string): number {
  let leftIndex = 0;
  let rightIndex = 0;
  let numericTieBreak = 0;
  while (leftIndex < left.length && rightIndex < right.length) {
    const leftDigit = isAsciiDigit(left.charCodeAt(leftIndex));
    const rightDigit = isAsciiDigit(right.charCodeAt(rightIndex));
    if (leftDigit && rightDigit) {
      const leftEnd = digitRunEnd(left, leftIndex);
      const rightEnd = digitRunEnd(right, rightIndex);
      const leftRun = left.slice(leftIndex, leftEnd);
      const rightRun = right.slice(rightIndex, rightEnd);
      const compared = compareDigitMagnitude(leftRun, rightRun);
      if (compared !== 0) return compared;
      if (numericTieBreak === 0) {
        numericTieBreak =
          leftRun.length - rightRun.length ||
          compareExactAscii(leftRun, rightRun);
      }
      leftIndex = leftEnd;
      rightIndex = rightEnd;
      continue;
    }
    if (leftDigit !== rightDigit) {
      return (
        asciiLowerCode(left.charCodeAt(leftIndex)) -
        asciiLowerCode(right.charCodeAt(rightIndex))
      );
    }
    const leftEnd = nonDigitRunEnd(left, leftIndex);
    const rightEnd = nonDigitRunEnd(right, rightIndex);
    const compared = compareAsciiCaseInsensitive(
      left.slice(leftIndex, leftEnd),
      right.slice(rightIndex, rightEnd),
    );
    if (compared !== 0) return compared;
    leftIndex = leftEnd;
    rightIndex = rightEnd;
  }
  if (leftIndex === left.length && rightIndex !== right.length) return -1;
  if (rightIndex === right.length && leftIndex !== left.length) return 1;
  return numericTieBreak;
}

function compareDigitMagnitude(left: string, right: string): number {
  const leftSignificant = significantDigits(left);
  const rightSignificant = significantDigits(right);
  if (leftSignificant.length !== rightSignificant.length) {
    return leftSignificant.length - rightSignificant.length;
  }
  const significantComparison = compareExactAscii(
    leftSignificant,
    rightSignificant,
  );
  return significantComparison;
}

function significantDigits(value: string): string {
  const withoutLeadingZeroes = value.replace(/^0+/, "");
  return withoutLeadingZeroes || "0";
}

function compareAsciiCaseInsensitive(left: string, right: string): number {
  const length = Math.min(left.length, right.length);
  for (let index = 0; index < length; index += 1) {
    const compared =
      asciiLowerCode(left.charCodeAt(index)) -
      asciiLowerCode(right.charCodeAt(index));
    if (compared !== 0) return compared;
  }
  return left.length - right.length;
}

function compareExactAscii(left: string, right: string): number {
  if (left === right) return 0;
  return left < right ? -1 : 1;
}

function asciiLower(value: string): string {
  let normalized = "";
  for (let index = 0; index < value.length; index += 1) {
    normalized += String.fromCharCode(asciiLowerCode(value.charCodeAt(index)));
  }
  return normalized;
}

function asciiLowerCode(code: number): number {
  return code >= 65 && code <= 90 ? code + 32 : code;
}

function isAsciiDigit(code: number): boolean {
  return code >= 48 && code <= 57;
}

function digitRunEnd(value: string, start: number): number {
  let index = start;
  while (index < value.length && isAsciiDigit(value.charCodeAt(index))) {
    index += 1;
  }
  return index;
}

function nonDigitRunEnd(value: string, start: number): number {
  let index = start;
  while (index < value.length && !isAsciiDigit(value.charCodeAt(index))) {
    index += 1;
  }
  return index;
}

function arrayMove<T>(values: readonly T[], from: number, to: number): T[] {
  const next = values.slice();
  const [item] = next.splice(from, 1);
  if (item === undefined) return next;
  next.splice(to, 0, item);
  return next;
}
