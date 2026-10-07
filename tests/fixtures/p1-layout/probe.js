(() => {
  const rectangle = id => {
    const node = document.getElementById(id);
    const rect = node.getBoundingClientRect();
    const parent = node.parentElement.getBoundingClientRect();
    return [rect.left - parent.left, rect.top - parent.top, rect.width, rect.height];
  };
  const ids = ['flex-a', 'flex-b', 'rows-a', 'rows-b', 'rows-c',
    'columns-a', 'columns-b', 'columns-c', 'sparse-c', 'dense-c',
    'negative-a', 'negative-b', 'negative-c'];
  const rectangles = {};
  for (const id of ids) rectangles[id] = rectangle(id);
  return {
    rectangles,
    hit: document.elementFromPoint(10, 10).id,
    order: Array.from(document.getElementById('flex').children).map(node => node.id),
    flow: getComputedStyle(document.getElementById('dense')).gridAutoFlow,
  };
})()
