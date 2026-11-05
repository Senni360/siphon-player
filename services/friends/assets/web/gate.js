const form = document.getElementById('encore-gate');
const problem = document.getElementById('problem');
form.addEventListener('submit', async event => {
  event.preventDefault();
  const button = form.querySelector('button');
  button.disabled = true;
  problem.hidden = true;
  try {
    const answer = await fetch('/gate', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ code: form.code.value }),
    });
    if (answer.ok) {
      location.reload();
      return;
    }
    problem.textContent = (await answer.json()).error;
  } catch {
    problem.textContent = "Couldn't reach Encore. Check your connection.";
  }
  problem.hidden = false;
  button.disabled = false;
  form.code.select();
});
